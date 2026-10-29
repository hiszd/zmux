//! Rendering primitives for zmux's terminal user interface.
//!
//! The UI is rendered in layers: [`layout`] reserves space for the top and
//! bottom bars and the PTY; [`draw`] then projects the Alacritty terminal grid
//! into a ratatui frame and paints overlays on top. It reads
//! the current terminal grid and writes it into the ratatui frame on every requested
//! frame. Ratatui performs the final output diff against the previous frame.
//!
//! A typical application-owned render loop looks like this:
//!
//! ```ignore
//! let view = UiRender {
//!     screen: terminal_screen,
//!     leader_mode: false,
//!     modal: None,
//!     which_key: WhichKeyPopup::default(),
//!     key_warning: None,
//!     options,
//!     lua: &lua_config,
//!     trace: &trace,
//! };
//! draw(frame, &view)?;
//! ```
//!
//! Text is measured with Unicode display width rather than byte length. This
//! matters for both wide characters and combining marks in bar and modal
//! content.

use std::time::Instant;

use alacritty_terminal::{
  Grid,
  grid::Dimensions,
  index::{
    Column,
    Line,
  },
  term::cell::{
    Cell,
    Flags,
  },
  vte::ansi::{
    Color as TerminalColor,
    NamedColor,
  },
};
use crossterm::cursor::SetCursorStyle;
use ratatui::{
  Frame,
  layout::{
    Position,
    Rect,
  },
  style::{
    Color,
    Modifier,
    Style,
  },
  widgets::{
    Block,
    Borders,
    Clear,
    Paragraph,
    Widget,
    Wrap,
  },
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{
  input::which_key::WhichKeyPopup,
  lua::{
    LuaConfig,
    UiOptions,
  },
  trace::Trace,
  ui::StyleRegistry,
};

/// The shape used by the terminal's cursor while a PTY is focused.
///
/// The value is independent of whether a terminal supports blinking; the
/// renderer deliberately selects the steady crossterm variants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorShape {
  #[default]
  /// A solid block occupying the cursor cell.
  Block,
  /// A horizontal underline at the bottom of the cursor cell.
  Underline,
  /// A vertical bar at the left edge of the cursor cell.
  Bar,
}

impl CursorShape {
  /// Converts this application setting to the crossterm cursor style.
  ///
  /// ```
  /// # use zmux::ui::CursorShape;
  /// use crossterm::cursor::SetCursorStyle;
  ///
  /// assert_eq!(CursorShape::Bar.terminal_style(), SetCursorStyle::SteadyBar);
  /// ```
  pub fn terminal_style(self) -> SetCursorStyle {
    match self {
      Self::Block => SetCursorStyle::SteadyBlock,
      Self::Underline => SetCursorStyle::SteadyUnderScore,
      Self::Bar => SetCursorStyle::SteadyBar,
    }
  }
}

impl std::str::FromStr for CursorShape {
  type Err = String;

  /// Parses `block`, `underline`, or `bar` from configuration text.
  ///
  /// Parsing is intentionally case-sensitive so configuration mistakes are
  /// reported instead of silently selecting a different cursor.
  fn from_str(value: &str) -> Result<Self, Self::Err> {
    match value {
      "block" => Ok(Self::Block),
      "underline" => Ok(Self::Underline),
      "bar" => Ok(Self::Bar),
      _ => Err(format!("invalid cursor_shape '{value}'; expected 'block', 'underline', or 'bar'")),
    }
  }
}

/// A centered modal containing an optional query, selectable rows, and footer.
///
/// Rows are vertically scrolled so `selected_row` remains visible. The modal
/// owns no state: callers update these references and render again after input.
pub struct ModalOverlay<'a> {
  /// Title displayed in the border.
  pub title: &'a str,
  /// Optional query line rendered before the rows.
  pub query: Option<&'a str>,
  /// Candidate rows rendered below the query.
  pub rows: &'a [String],
  /// Row to highlight and keep visible, if any.
  pub selected_row: Option<usize>,
  /// Help or status text pinned to the bottom.
  pub footer: &'a str,
}

/// All application state required for one [`draw`] call.
///
/// This is a short-lived, immutable view assembled by the application. The
/// complete current view is rendered whenever the event loop requests a frame;
/// ratatui handles the final terminal-buffer diff.
pub struct UiRender<'a> {
  /// Parsed terminal contents to project into the PTY pane.
  pub screen: &'a Grid<Cell>,
  /// Whether the terminal cursor is visible.
  pub cursor_visible: bool,
  /// Whether the leader key mode indicator is active.
  pub leader_mode: bool,
  /// Optional modal drawn over the panel.
  pub modal: Option<ModalOverlay<'a>>,
  /// Which-key popup shown while leader input is active.
  pub which_key: WhichKeyPopup,
  /// Optional warning drawn over the bottom-most visible Bar.
  pub key_warning: Option<&'a str>,
  /// Optional temporary configuration reload notification.
  pub reload_warning: Option<&'a str>,
  /// UI layout and PTY style options.
  pub options: UiOptions,
  /// Lua configuration that owns and renders UI elements.
  pub lua: &'a LuaConfig,
  /// Frame timing and state trace sink.
  pub trace: &'a Trace,
}

/// Rectangles produced by [`layout`] for each UI layer.
///
/// `pty` includes its border; `pane` is the interior where terminal cells are
/// copied. The two anchor rectangles are the top and bottom bars.
pub struct UiLayout {
  /// Full rectangle reserved for the top bar.
  pub top_anchor: Rect,
  /// Full rectangle reserved for the bottom bar.
  pub bottom_anchor: Rect,
  /// Area between the anchors.
  pub panel: Rect,
  /// PTY rectangle, including its border.
  pub pty: Rect,
  /// PTY interior where terminal cells are rendered.
  pub pane: Rect,
}

/// Returns cursor visibility and coordinates relative to the PTY pane.
///
/// The cursor is derived directly from the current Alacritty grid. Coordinates
/// outside the pane are retained with `visible == false` so the caller can
/// decide whether to place a ratatui cursor.
fn cursor_state(
  screen: &Grid<Cell>,
  cursor_visible: bool,
  width: u16,
  height: u16,
) -> Option<(bool, u16, u16)> {
  let point = screen.cursor.point;
  let row = point.line.0.max(0) as u16;
  let column = point.column.0 as u16;
  let visible =
    cursor_visible && point.line.0 >= 0 && point.line.0 < height as i32 && column < width;
  Some((visible, row, column))
}

/// Renders every cell in the current Alacritty grid into the PTY pane.
///
/// This is intentionally immediate-mode: it does not retain cell state or
/// perform its own invalidation. The event loop decides whether a frame is
/// needed, and ratatui compares the completed frame with its previous buffer.
fn render_screen(buffer: &mut ratatui::buffer::Buffer, area: Rect, screen: &Grid<Cell>) {
  for row in 0..area.height {
    for column in 0..area.width {
      let (symbol, style) =
        if row < screen.screen_lines() as u16 && column < screen.columns() as u16 {
          let cell = &screen[Line(row as i32)][Column(column as usize)];
          let symbol = if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            " ".to_owned()
          } else {
            cell.c.to_string()
          };
          (symbol, cell_style(cell))
        } else {
          (" ".to_owned(), Style::default())
        };
      buffer[(area.x + column, area.y + row)]
        .set_symbol(&symbol)
        .set_style(style);
    }
  }
}

/// Computes the rectangles used by the UI.
///
/// Visible Bars reserve one row each, up to the available area. The PTY uses
/// the full width between the top and bottom anchors; other UI Elements are
/// overlays and do not affect its geometry.
///
/// ```
/// # use zmux::ui::layout;
/// # use zmux::lua::UiOptions;
/// # use ratatui::layout::Rect;
/// let options = UiOptions {
///     top_bars: 1,
///     bottom_bars: 1,
///     ..UiOptions::default()
/// };
/// let parts = layout(Rect::new(0, 0, 80, 24), options);
/// assert_eq!((parts.pane.width, parts.pane.height), (78, 21));
/// ```
pub fn layout(area: Rect, options: UiOptions) -> UiLayout {
  let top_height = options.top_bars.min(area.height);
  let bottom_height = options
    .bottom_bars
    .min(area.height.saturating_sub(top_height));
  let panel_height = area.height.saturating_sub(top_height + bottom_height);
  let panel = Rect::new(area.x, area.y + top_height, area.width, panel_height);
  let top_anchor = Rect::new(area.x, area.y, area.width, top_height);
  let bottom_anchor =
    Rect::new(area.x, area.y + top_height + panel_height, area.width, bottom_height);
  let pty = panel;
  let pane = Block::default()
    .borders(options.pty.border.borders())
    .inner(pty);
  UiLayout {
    top_anchor,
    bottom_anchor,
    panel,
    pty,
    pane,
  }
}

/// Returns the PTY's inner `(height, width)` for a terminal resize request.
///
/// The order matches the common PTY resize API rather than [`ratatui::layout::Rect`].
pub fn pane_size(width: u16, height: u16, options: UiOptions) -> (u16, u16) {
  let layout = layout(Rect::new(0, 0, width, height), options);
  (layout.pane.height, layout.pane.width)
}

/// Renders one complete UI frame.
///
/// The sequence is significant: retained Bars and the PTY are painted first,
/// then UI overlays, the modal, and the which-key popup. Overlays can obscure
/// terminal content without changing the terminal model.
pub fn draw(frame: &mut Frame<'_>, view: &UiRender<'_>) -> mlua::Result<()> {
  let frame_started = Instant::now();
  let area = frame.area();
  let layout = layout(area, view.options.clone());
  let rect_detail = |rect: Option<Rect>| {
    rect.map_or_else(
      || "none".to_owned(),
      |rect| format!("x={} y={} w={} h={}", rect.x, rect.y, rect.width, rect.height),
    )
  };
  view.trace.mark(&format!(
    "ui-frame-layout area={}x{} top_bars={} bottom_bars={} pty={} pane={}",
    area.width,
    area.height,
    view.options.top_bars,
    view.options.bottom_bars,
    rect_detail(Some(layout.pty)),
    rect_detail(Some(layout.pane)),
  ));
  let which_key_area = if view.modal.is_none() && view.leader_mode {
    view.which_key.popup_area(area)
  } else {
    None
  };
  let which_key_visible = which_key_area.is_some();
  view.trace.mark(&format!(
    "ui-overlay-state which_key_visible={} which_key={}",
    which_key_visible,
    rect_detail(which_key_area),
  ));
  let screen_started = Instant::now();
  let screen = view.screen;
  let buffer_started = Instant::now();
  {
    let buffer = frame.buffer_mut();
    view
      .lua
      .render_ui_bars(layout.top_anchor, layout.bottom_anchor, buffer);
    if let Some(warning) = view.key_warning
      && !layout.bottom_anchor.is_empty()
    {
      let text = format!(" {warning} ");
      let width =
        UnicodeWidthStr::width(text.as_str()).min(usize::from(layout.bottom_anchor.width)) as u16;
      let x = layout
        .bottom_anchor
        .x
        .saturating_add(layout.bottom_anchor.width.saturating_sub(width));
      let y = layout.bottom_anchor.y + layout.bottom_anchor.height - 1;
      draw_styled_text(
        buffer,
        layout.bottom_anchor,
        y,
        x,
        &text,
        view.lua.resolve_style("Warning.Key"),
      );
    }
    Block::default()
      .borders(view.options.pty.border.borders())
      .border_type(view.options.pty.border.ratatui_type())
      .border_style(
        view
          .lua
          .resolve_style_source(&view.options.pty.border_style),
      )
      .render(layout.pty, buffer);
    render_screen(buffer, layout.pane, screen);
    view.lua.render_ui_elements(layout.panel, buffer);
    if let Some(modal) = &view.modal {
      modal.render(layout.panel, buffer, &view.lua.style_registry());
    }
  }
  let buffer_duration = buffer_started.elapsed();
  view.trace.duration(
    "ui-terminal-screen-render",
    screen_started.elapsed(),
    &format!("area={}x{}", layout.pane.width, layout.pane.height),
  );
  let cursor = cursor_state(screen, view.cursor_visible, layout.pane.width, layout.pane.height);
  if which_key_visible {
    view
      .which_key
      .render(area, frame.buffer_mut(), &view.lua.style_registry());
  }
  if let Some(warning) = view.reload_warning {
    render_reload_warning(area, warning, frame.buffer_mut(), &view.lua.style_registry());
  }
  if view.modal.is_none()
    && let Some((true, row, column)) = cursor
  {
    frame.set_cursor_position(Position::new(layout.pane.x + column, layout.pane.y + row));
  }

  view.trace.duration(
    "ui-buffer-populate",
    buffer_duration,
    &format!("cells={}", usize::from(layout.pane.width) * usize::from(layout.pane.height)),
  );
  view.trace.duration(
    "ui-frame-closure",
    frame_started.elapsed(),
    &format!("area={}x{}", area.width, area.height),
  );

  Ok(())
}

fn render_reload_warning(
  area: Rect,
  warning: &str,
  buffer: &mut ratatui::buffer::Buffer,
  styles: &StyleRegistry,
) {
  if area.is_empty() {
    return;
  }
  let width = UnicodeWidthStr::width(warning)
    .saturating_add(2)
    .clamp(24, 64)
    .min(usize::from(area.width)) as u16;
  let width = width.max(1);
  let height = 4.min(area.height);
  let popup =
    Rect::new(area.x.saturating_add(area.width.saturating_sub(width)), area.y, width, height);
  Clear.render(popup, buffer);
  Paragraph::new(warning)
    .wrap(Wrap { trim: true })
    .block(
      Block::default()
        .title("Config reload failed")
        .borders(Borders::ALL)
        .border_style(styles.resolve("Warning.Reload")),
    )
    .render(popup, buffer);
}

impl ModalOverlay<'_> {
  /// Sizes the modal to its content, centers it, and scrolls rows around the
  /// selected entry when the available height is too small.
  pub fn render(&self, area: Rect, buffer: &mut ratatui::buffer::Buffer, styles: &StyleRegistry) {
    let modal = self;
    if area.is_empty() {
      return;
    }
    let widest_row = modal
      .rows
      .iter()
      .map(|row| UnicodeWidthStr::width(row.as_str()))
      .max()
      .unwrap_or(0);
    let width = widest_row
      .max(UnicodeWidthStr::width(modal.title))
      .max(UnicodeWidthStr::width(modal.footer))
      .max(
        modal
          .query
          .map_or(0, |query| UnicodeWidthStr::width(query) + 2),
      )
      .saturating_add(8)
      .min(usize::from(area.width)) as u16;
    let desired_height = modal.rows.len() + usize::from(modal.query.is_some()) + 3;
    let height = desired_height.max(7).min(usize::from(area.height)) as u16;
    if width == 0 || height == 0 {
      return;
    }
    let modal_area = Rect::new(
      area.x + area.width.saturating_sub(width) / 2,
      area.y + area.height.saturating_sub(height) / 2,
      width,
      height,
    );
    Clear.render(modal_area, buffer);
    let block = Block::default()
      .borders(Borders::ALL)
      .title(modal.title)
      .border_style(styles.resolve("Modal.Border"))
      .style(styles.resolve("Modal.Background"));
    let inner = block.inner(modal_area);
    block.render(modal_area, buffer);

    let mut y = inner.y;
    if let Some(query) = modal.query
      && y < inner.y.saturating_add(inner.height)
    {
      draw_styled_text(
        buffer,
        inner,
        y,
        inner.x,
        &format!("> {query}"),
        styles.resolve("Modal.Query"),
      );
      y += 1;
    }
    let bottom = inner.y.saturating_add(inner.height);
    let rows_available = bottom.saturating_sub(y).saturating_sub(1) as usize;
    let start = if modal.rows.len() > rows_available && rows_available > 0 {
      modal
        .selected_row
        .unwrap_or_default()
        .saturating_sub(rows_available - 1)
        .min(modal.rows.len() - rows_available)
    } else {
      0
    };
    for (index, text) in modal
      .rows
      .iter()
      .enumerate()
      .skip(start)
      .take(rows_available)
    {
      let style = if modal.selected_row == Some(index) {
        styles.resolve("Modal.Selected")
      } else {
        styles.resolve("Modal.Item")
      };
      draw_styled_text(buffer, inner, y, inner.x, text, style);
      y += 1;
    }
    if y < bottom {
      draw_styled_text(
        buffer,
        inner,
        bottom - 1,
        inner.x,
        modal.footer,
        styles.resolve("Modal.Footer"),
      );
    }
  }
}

/// Draws styled text within a rectangular area, clipping at its right edge.
///
/// Zero-width graphemes (for example combining accents) are appended to the
/// preceding cell rather than consuming another terminal column.
fn draw_styled_text(
  buffer: &mut ratatui::buffer::Buffer,
  area: Rect,
  y: u16,
  mut x: u16,
  text: &str,
  style: Style,
) {
  let end = area.x.saturating_add(area.width);
  for grapheme in text.graphemes(true) {
    let width = UnicodeWidthStr::width(grapheme) as u16;
    if width == 0 {
      if x > area.x {
        let previous = buffer[(x - 1, y)].symbol().to_owned();
        buffer[(x - 1, y)].set_symbol(&format!("{previous}{grapheme}"));
      }
      continue;
    }
    if x.saturating_add(width) > end {
      return;
    }
    let inherited = buffer[(x, y)].style();
    buffer[(x, y)]
      .set_symbol(grapheme)
      .set_style(inherited.patch(style));
    x += width;
  }
}

fn cell_style(cell: &Cell) -> Style {
  let mut modifiers = Modifier::empty();
  if cell.flags.contains(Flags::BOLD) {
    modifiers.insert(Modifier::BOLD);
  }
  if cell.flags.contains(Flags::ITALIC) {
    modifiers.insert(Modifier::ITALIC);
  }
  if cell.flags.intersects(Flags::ALL_UNDERLINES) {
    modifiers.insert(Modifier::UNDERLINED);
  }
  if cell.flags.contains(Flags::INVERSE) {
    modifiers.insert(Modifier::REVERSED);
  }
  if cell.flags.contains(Flags::HIDDEN) {
    modifiers.insert(Modifier::HIDDEN);
  }
  if cell.flags.contains(Flags::STRIKEOUT) {
    modifiers.insert(Modifier::CROSSED_OUT);
  }
  Style::default()
    .fg(color(cell.fg))
    .bg(color(cell.bg))
    .add_modifier(modifiers)
}

fn color(value: TerminalColor) -> Color {
  match value {
    TerminalColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
    TerminalColor::Indexed(value) => Color::Indexed(value),
    TerminalColor::Named(name) => match name {
      NamedColor::Foreground
      | NamedColor::BrightForeground
      | NamedColor::DimForeground
      | NamedColor::Background
      | NamedColor::Cursor => Color::Reset,
      _ => Color::Indexed(name as u8),
    },
  }
}

#[allow(dead_code)]
fn _area(_: Rect) {
}

#[cfg(test)]
mod tests {
  use ratatui::{
    buffer::Buffer,
    layout::Rect,
  };

  use super::{
    StyleRegistry,
    UiOptions,
    layout,
    pane_size,
    render_reload_warning,
    render_screen,
  };
  use crate::terminal_emulator::TerminalEmulator;

  // Developer approved: do not remove.
  #[test]
  fn anchors_reserve_rows_without_a_workspace_drawer() {
    let options = UiOptions {
      top_bars: 2,
      bottom_bars: 3,
      ..UiOptions::default()
    };
    let parts = layout(Rect::new(0, 0, 100, 30), options);
    assert_eq!(parts.top_anchor.height, 2);
    assert_eq!(parts.bottom_anchor.height, 3);
    assert_eq!(parts.pane.width, 98);
    assert_eq!(parts.pane.height, 23);
  }

  // Developer approved: do not remove.
  #[test]
  fn drawer_overlays_do_not_reduce_the_pty_viewport() {
    let options = UiOptions::default();
    assert_eq!(pane_size(80, 24, options), (22, 78));
  }

  #[test]
  fn reload_warnings_render_as_a_bordered_popup() {
    let area = Rect::new(0, 0, 40, 8);
    let mut buffer = Buffer::empty(area);
    let styles = StyleRegistry::default();

    render_reload_warning(area, "invalid Lua config", &mut buffer, &styles);

    let rendered = buffer
      .content()
      .iter()
      .map(|cell| cell.symbol())
      .collect::<String>();
    assert!(rendered.contains("Config reload failed"));
    assert!(rendered.contains("invalid Lua config"));
  }

  #[test]
  fn disabling_the_pty_border_uses_the_full_panel_area() {
    let options = UiOptions {
      pty: crate::lua::PtyOptions {
        border: crate::ui::DrawerBorder::None,
        ..UiOptions::default().pty
      },
      ..UiOptions::default()
    };
    let parts = layout(Rect::new(0, 0, 80, 24), options.clone());
    assert_eq!((parts.pane.width, parts.pane.height), (80, 24));
    assert_eq!(pane_size(80, 24, options), (24, 80));
  }

  // Developer approved: do not remove.
  #[test]
  fn screen_render_projects_current_contents_and_erasure() {
    let mut parser = TerminalEmulator::new(2, 4);
    parser
      .process(b"x", &mut std::io::sink())
      .expect("parse output");
    let area = Rect::new(0, 0, 5, 2);
    let mut buffer = Buffer::empty(area);

    render_screen(&mut buffer, area, parser.grid());
    assert_eq!(buffer[(0, 0)].symbol(), "x");

    parser
      .process(b"\x1b[1;1H\x1b[K", &mut std::io::sink())
      .expect("parse output");
    render_screen(&mut buffer, area, parser.grid());
    assert_eq!(buffer[(0, 0)].symbol(), " ");
  }
}
