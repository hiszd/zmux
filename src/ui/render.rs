//! Rendering primitives for zmux's terminal user interface.
//!
//! The UI is rendered in layers: [`layout`] reserves space for the top and
//! bottom bars, the workspace drawer, and the PTY; [`draw`] then projects the
//! Alacritty's terminal grid into a ratatui frame and paints overlays on top. It reads
//! the current terminal grid and writes it into the ratatui frame on every requested
//! frame. Ratatui performs the final output diff against the previous frame.
//!
//! A typical application-owned render loop looks like this:
//!
//! ```ignore
//! let view = UiRender {
//!     screen: terminal_screen,
//!     leader_mode: false,
//!     tab_count: 2,
//!     active_tab: 0,
//!     active_title: "shell",
//!     active_exit_code: None,
//!     workspace_drawer: WorkspaceDrawerRender {
//!         expanded: false,
//!         active_id: 1,
//!         highlighted_id: 1,
//!         items: &workspaces,
//!     },
//!     modal: None,
//!     which_key: WhichKeyPopup::default(),
//!     key_warning: None,
//!     options,
//!     bars: BarContents::default(),
//!     lua: &lua_config,
//!     trace: &trace,
//! };
//! draw(frame, &view)?;
//! ```
//!
//! Text is measured with Unicode display width rather than byte length. This
//! matters for both wide characters and combining marks in bar, modal, and
//! drawer content.

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
    Widget,
  },
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{
  input::which_key::WhichKeyPopup,
  lua::{
    BarSegment,
    LuaConfig,
    UiOptions,
  },
  trace::Trace,
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

/// The three horizontally aligned groups in one status bar.
///
/// `None` means “use the built-in group” when passed through [`draw`]. `Some`
/// with an empty vector means “intentionally render nothing”. Each segment's
/// text may contain newlines; the corresponding line is selected while the bar
/// is painted.
#[derive(Clone, Default)]
pub struct BarParts {
  /// Segments drawn from the left edge.
  pub left: Option<Vec<BarSegment>>,
  /// Segments centered in the bar.
  pub center: Option<Vec<BarSegment>>,
  /// Segments drawn from the right edge.
  pub right: Option<Vec<BarSegment>>,
}

/// Top and bottom status-bar content supplied to [`draw`].
#[derive(Clone, Default)]
pub struct BarContents {
  /// Content for the top anchor.
  pub top: BarParts,
  /// Content for the bottom anchor.
  pub bottom: BarParts,
}

/// One workspace entry shown by [`WorkspaceDrawerRender`].
pub struct WorkspaceDrawerItem<'a> {
  /// Stable workspace identifier used for active/highlight matching.
  pub id: u64,
  /// User-facing workspace name.
  pub name: &'a str,
}

/// State and entries needed to paint the workspace drawer.
///
/// In collapsed mode only the first single-width grapheme of each name is
/// displayed. In expanded mode the highlighted item follows keyboard
/// navigation while `active_id` identifies the currently selected workspace.
pub struct WorkspaceDrawerRender<'a> {
  /// Whether to show full names instead of initials.
  pub expanded: bool,
  /// ID of the workspace currently in use.
  pub active_id: u64,
  /// ID selected by navigation while expanded.
  pub highlighted_id: u64,
  /// Workspaces to display in order.
  pub items: &'a [WorkspaceDrawerItem<'a>],
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
  /// Number of tabs represented by the default top bar.
  pub tab_count: usize,
  /// Zero-based active tab index.
  pub active_tab: usize,
  /// Title used by the default bottom bar.
  pub active_title: &'a str,
  /// Exit code shown when the active terminal has exited.
  pub active_exit_code: Option<u32>,
  /// Workspace drawer state.
  pub workspace_drawer: WorkspaceDrawerRender<'a>,
  /// Optional modal drawn over the panel.
  pub modal: Option<ModalOverlay<'a>>,
  /// Which-key popup shown while leader input is active.
  pub which_key: WhichKeyPopup,
  /// Optional warning replacing the default right bottom-bar group.
  pub key_warning: Option<&'a str>,
  /// Visibility and height settings for the anchors.
  pub options: UiOptions,
  /// User-supplied top and bottom bar groups.
  pub bars: BarContents,
  /// Lua configuration used to resolve segment styles.
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
  /// Rectangle available to the workspace drawer.
  pub drawer: Rect,
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
/// Anchor heights are clamped to the available area. The collapsed drawer is
/// one cell wide; the expanded drawer overlays the panel without changing the
/// PTY geometry.
///
/// ```
/// # use zmux::ui::layout;
/// # use zmux::lua::{AnchorOptions, UiOptions};
/// # use ratatui::layout::Rect;
/// let options = UiOptions {
///     top: AnchorOptions { height: 1, visible: true },
///     bottom: AnchorOptions { height: 1, visible: true },
///     ..UiOptions::default()
/// };
/// let parts = layout(Rect::new(0, 0, 80, 24), options, false);
/// assert_eq!((parts.pane.width, parts.pane.height), (77, 21));
/// ```
pub fn layout(area: Rect, options: UiOptions, drawer_expanded: bool) -> UiLayout {
  let top_height = if options.top.visible {
    options.top.height.min(area.height)
  } else {
    0
  };
  let bottom_height = if options.bottom.visible {
    options
      .bottom
      .height
      .min(area.height.saturating_sub(top_height))
  } else {
    0
  };
  let panel_height = area.height.saturating_sub(top_height + bottom_height);
  let panel = Rect::new(area.x, area.y + top_height, area.width, panel_height);
  let top_anchor = Rect::new(area.x, area.y, area.width, top_height);
  let bottom_anchor =
    Rect::new(area.x, area.y + top_height + panel_height, area.width, bottom_height);
  let pty_drawer_width = drawer_width(panel.width, false);
  let drawer = if drawer_expanded {
    panel
  } else {
    Rect::new(panel.x, panel.y, pty_drawer_width, panel.height)
  };
  let pty = Rect::new(
    panel.x.saturating_add(pty_drawer_width),
    panel.y,
    panel.width.saturating_sub(pty_drawer_width),
    panel.height,
  );
  let pane = Block::default().borders(Borders::ALL).inner(pty);
  UiLayout {
    top_anchor,
    bottom_anchor,
    panel,
    drawer,
    pty,
    pane,
  }
}

/// Returns the PTY's inner `(height, width)` for a terminal resize request.
///
/// The order matches the common PTY resize API rather than [`ratatui::layout::Rect`].
pub fn pane_size(width: u16, height: u16, options: UiOptions, drawer_expanded: bool) -> (u16, u16) {
  let layout = layout(Rect::new(0, 0, width, height), options, drawer_expanded);
  (layout.pane.height, layout.pane.width)
}

/// Computes the drawer width used by [`layout`].
///
/// A collapsed drawer always reserves one column when possible. Expanded mode
/// deliberately consumes the whole panel so the drawer can display a focused
/// workspace list without a partially visible PTY behind it.
fn drawer_width(inner_width: u16, expanded: bool) -> u16 {
  if inner_width == 0 {
    return 0;
  }
  if expanded {
    // Two cells for the horizontal padding and one for the right border.
    inner_width
  } else {
    // One cell for the collapsed initial. The PTY border provides the
    // separation from the drawer.
    1.min(inner_width)
  }
}

/// Renders one complete UI frame.
///
/// The sequence is significant: bars and the PTY are painted first, then the
/// drawer and modal, and finally the which-key popup and cursor. Overlays can
/// therefore obscure terminal content without changing the terminal model.
/// Lua-configured bar styles are applied before painting. A Lua error is
/// returned to the caller so the event loop can decide whether it is
/// recoverable.
pub fn draw(frame: &mut Frame<'_>, view: &UiRender<'_>) -> mlua::Result<()> {
  let frame_started = Instant::now();
  let area = frame.area();
  let layout = layout(area, view.options, view.workspace_drawer.expanded);
  let rect_detail = |rect: Option<Rect>| {
    rect.map_or_else(
      || "none".to_owned(),
      |rect| format!("x={} y={} w={} h={}", rect.x, rect.y, rect.width, rect.height),
    )
  };
  view.trace.mark(&format!(
    "ui-frame-layout area={}x{} drawer_expanded={} pty={} pane={}",
    area.width,
    area.height,
    view.workspace_drawer.expanded,
    rect_detail(Some(layout.pty)),
    rect_detail(Some(layout.pane)),
  ));
  let which_key_area = if view.modal.is_none() && view.leader_mode {
    view.which_key.layout(area).map(|layout| layout.outer)
  } else {
    None
  };
  let which_key_visible = which_key_area.is_some();
  view.trace.mark(&format!(
    "ui-overlay-state which_key_visible={} which_key={}",
    which_key_visible,
    rect_detail(which_key_area),
  ));
  let mut top_contents = defaulted_parts(
    view.bars.top.clone(),
    default_top_parts(view.tab_count, view.active_tab, view.leader_mode),
  );
  let mut bottom_contents = defaulted_parts(
    view.bars.bottom.clone(),
    default_bottom_parts(view.active_title, view.active_exit_code, area.width, area.height),
  );
  apply_styles(view.lua, &mut top_contents)?;
  apply_styles(view.lua, &mut bottom_contents)?;
  if let Some(warning) = view.key_warning {
    bottom_contents.right = Some(vec![BarSegment {
      text: format!(" {warning} "),
      role: None,
      style: Some(
        Style::default()
          .fg(Color::Yellow)
          .add_modifier(Modifier::BOLD),
      ),
    }]);
  }
  let screen_started = Instant::now();
  let screen = view.screen;
  let buffer_started = Instant::now();
  {
    let buffer = frame.buffer_mut();
    if !layout.top_anchor.is_empty() {
      draw_bar(buffer, layout.top_anchor, &top_contents);
    }
    if !layout.bottom_anchor.is_empty() {
      draw_bar(buffer, layout.bottom_anchor, &bottom_contents);
    }
    Block::default()
      .borders(Borders::ALL)
      .render(layout.pty, buffer);
    render_screen(buffer, layout.pane, screen);
    (&view.workspace_drawer).render(layout.drawer, buffer);
    if let Some(modal) = &view.modal {
      modal.render(layout.panel, buffer);
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
    (&view.which_key).render(area, frame.buffer_mut());
  }
  if !view.workspace_drawer.expanded
    && view.modal.is_none()
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

impl Widget for &ModalOverlay<'_> {
  /// Sizes the modal to its content, centers it, and scrolls rows around the
  /// selected entry when the available height is too small.
  fn render(self, area: Rect, buffer: &mut ratatui::buffer::Buffer) {
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
      .style(Style::default().fg(Color::Gray).bg(Color::Black));
    let inner = block.inner(modal_area);
    block.render(modal_area, buffer);

    let mut y = inner.y;
    if let Some(query) = modal.query
      && y < inner.y.saturating_add(inner.height)
    {
      draw_segments(buffer, inner, y, inner.x, &[BarSegment {
        text: format!("> {query}"),
        role: None,
        style: Some(Style::default().fg(Color::Yellow).bg(Color::Black)),
      }]);
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
        Style::default()
          .fg(Color::Black)
          .bg(Color::Cyan)
          .add_modifier(Modifier::BOLD)
      } else {
        Style::default().fg(Color::White).bg(Color::Black)
      };
      draw_segments(buffer, inner, y, inner.x, &[BarSegment {
        text: text.clone(),
        role: None,
        style: Some(style),
      }]);
      y += 1;
    }
    if y < bottom {
      draw_segments(buffer, inner, bottom - 1, inner.x, &[BarSegment {
        text: modal.footer.to_owned(),
        role: None,
        style: Some(Style::default().fg(Color::Gray).bg(Color::Black)),
      }]);
    }
  }
}

impl Widget for &WorkspaceDrawerRender<'_> {
  /// Paints the drawer and scrolls it so its active/highlighted entry stays
  /// visible. Unicode grapheme boundaries are respected for collapsed initials.
  fn render(self, pane: Rect, buffer: &mut ratatui::buffer::Buffer) {
    let drawer = self;
    if pane.is_empty() || drawer.items.is_empty() {
      return;
    }
    let width = if drawer.expanded {
      drawer
        .items
        .iter()
        .map(|item| UnicodeWidthStr::width(item.name))
        .max()
        .unwrap_or(0)
        .saturating_add(3)
        .min(usize::from(pane.width)) as u16
    } else {
      1.min(pane.width)
    };
    if width == 0 {
      return;
    }
    let area = Rect::new(pane.x, pane.y, width, pane.height);
    let background = Style::default().bg(Color::Black);
    for y in area.y..area.y.saturating_add(area.height) {
      for x in area.x..area.x.saturating_add(area.width) {
        buffer[(x, y)].set_symbol(" ").set_style(background);
      }
    }
    let block = if drawer.expanded {
      Block::default()
        .borders(Borders::RIGHT)
        .style(Style::default().fg(Color::Gray).bg(Color::Black))
    } else {
      Block::default().style(background)
    };
    let inner = block.inner(area);
    block.render(area, buffer);

    let highlight_id = if drawer.expanded {
      drawer.highlighted_id
    } else {
      drawer.active_id
    };
    let highlight_index = drawer
      .items
      .iter()
      .position(|item| item.id == highlight_id)
      .unwrap_or_default();
    let visible_rows = usize::from(inner.height);
    let start = if drawer.items.len() > visible_rows {
      highlight_index
        .saturating_sub(visible_rows.saturating_sub(1))
        .min(drawer.items.len() - visible_rows)
    } else {
      0
    };

    for (row, item) in drawer
      .items
      .iter()
      .enumerate()
      .skip(start)
      .take(visible_rows)
    {
      let y = inner.y + (row - start) as u16;
      let is_active = item.id == drawer.active_id;
      let is_highlighted = drawer.expanded && item.id == drawer.highlighted_id;
      let style = if is_highlighted {
        Style::default()
          .fg(Color::Black)
          .bg(Color::Yellow)
          .add_modifier(Modifier::BOLD)
      } else if is_active {
        Style::default()
          .fg(Color::Black)
          .bg(Color::Cyan)
          .add_modifier(Modifier::BOLD)
      } else {
        Style::default().fg(Color::Gray).bg(Color::Black)
      };
      let text = if drawer.expanded {
        format!(" {} ", item.name)
      } else {
        let initial = item
          .name
          .graphemes(true)
          .next()
          .filter(|grapheme| UnicodeWidthStr::width(*grapheme) == 1)
          .unwrap_or(" ");
        initial.to_owned()
      };
      draw_segments(buffer, inner, y, inner.x, &[BarSegment {
        text,
        role: None,
        style: Some(style),
      }]);
    }
  }
}

/// Resolves Lua-defined default styles for every segment in a bar.
fn apply_styles(lua: &LuaConfig, parts: &mut BarParts) -> mlua::Result<()> {
  for segment in parts
    .left
    .iter_mut()
    .flatten()
    .chain(parts.center.iter_mut().flatten())
    .chain(parts.right.iter_mut().flatten())
  {
    lua.apply_default_style(segment)?;
  }
  Ok(())
}

/// Fills only missing bar groups, preserving explicitly empty groups.
fn defaulted_parts(mut configured: BarParts, defaults: BarParts) -> BarParts {
  if configured.left.is_none() {
    configured.left = defaults.left;
  }
  if configured.center.is_none() {
    configured.center = defaults.center;
  }
  if configured.right.is_none() {
    configured.right = defaults.right;
  }
  configured
}

/// Builds the built-in tab and leader indicators for the top bar.
fn default_top_parts(tab_count: usize, active_tab: usize, leader_mode: bool) -> BarParts {
  let tabs = (1..=tab_count)
    .map(|index| BarSegment {
      text: format!(" {index} "),
      role: Some(if index == active_tab + 1 {
        "selected_tab".to_owned()
      } else {
        "tab".to_owned()
      }),
      style: None,
    })
    .collect();
  BarParts {
    left: Some(tabs),
    right: leader_mode.then(|| {
      vec![BarSegment {
        text: " LEADER ".to_owned(),
        role: Some("leader".to_owned()),
        style: None,
      }]
    }),
    ..BarParts::default()
  }
}

/// Builds the built-in active-terminal and terminal-size indicators.
fn default_bottom_parts(
  active_title: &str,
  exit_code: Option<u32>,
  width: u16,
  height: u16,
) -> BarParts {
  let status = exit_code.map_or_else(String::new, |code| format!(" exited ({code})"));
  BarParts {
    left: Some(vec![BarSegment {
      text: format!(" {}{} ", active_title, status),
      role: Some("active_terminal".to_owned()),
      style: None,
    }]),
    right: Some(vec![BarSegment {
      text: format!(" {width}x{height} "),
      role: Some("terminal_size".to_owned()),
      style: None,
    }]),
    ..BarParts::default()
  }
}

/// Clears and paints all bar rows, aligning left, center, and right groups.
fn draw_bar(buffer: &mut ratatui::buffer::Buffer, area: Rect, parts: &BarParts) {
  for row in area.y..area.y + area.height {
    for column in area.x..area.x + area.width {
      buffer[(column, row)]
        .set_symbol(" ")
        .set_style(Style::default());
    }
  }
  for row in 0..area.height {
    let y = area.y + row;
    let left = line_segments(parts.left.as_deref(), row);
    let center = line_segments(parts.center.as_deref(), row);
    let right = line_segments(parts.right.as_deref(), row);
    let left_width = segments_width(&left);
    let center_width = segments_width(&center);
    let right_width = segments_width(&right);
    draw_segments(buffer, area, y, area.x, &left);
    let center_x = area.x + area.width.saturating_sub(center_width) / 2;
    draw_segments(buffer, area, y, center_x, &center);
    let right_x = area.x + area.width.saturating_sub(right_width);
    draw_segments(buffer, area, y, right_x.max(area.x + left_width), &right);
  }
}

/// Selects one line from each possibly multiline segment.
fn line_segments(segments: Option<&[BarSegment]>, line: u16) -> Vec<BarSegment> {
  segments
    .unwrap_or_default()
    .iter()
    .filter_map(|segment| {
      let text = segment.text.lines().nth(usize::from(line))?;
      Some(BarSegment {
        text: text.to_owned(),
        role: segment.role.clone(),
        style: segment.style,
      })
    })
    .collect()
}

/// Calculates display width using terminal columns, saturating at `u16::MAX`.
fn segments_width(segments: &[BarSegment]) -> u16 {
  segments
    .iter()
    .map(|segment| UnicodeWidthStr::width(segment.text.as_str()).min(usize::from(u16::MAX)) as u16)
    .fold(0_u16, u16::saturating_add)
}

/// Draws segments left-to-right, clipping at the area's right edge.
///
/// Zero-width graphemes (for example combining accents) are appended to the
/// preceding cell rather than consuming another terminal column.
fn draw_segments(
  buffer: &mut ratatui::buffer::Buffer,
  area: Rect,
  y: u16,
  mut x: u16,
  segments: &[BarSegment],
) {
  let end = area.x.saturating_add(area.width);
  for segment in segments {
    let style = segment.style.unwrap_or_else(|| default_style(segment));
    for grapheme in segment.text.graphemes(true) {
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
      buffer[(x, y)].set_symbol(grapheme).set_style(style);
      x += width;
    }
  }
}

/// Supplies the built-in style for a segment without an explicit style.
fn default_style(segment: &BarSegment) -> Style {
  match segment.role.as_deref() {
    Some("selected_tab") => Style::default()
      .fg(Color::Yellow)
      .add_modifier(Modifier::BOLD),
    Some("leader") => Style::default()
      .fg(Color::Black)
      .bg(Color::Yellow)
      .add_modifier(Modifier::BOLD),
    _ => Style::default().fg(Color::Gray),
  }
}

/// Converts an empty terminal cell into a visible blank terminal cell.
#[cfg(test)]
fn display_symbol(contents: &str) -> &str { if contents.is_empty() { " " } else { contents } }

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
    widgets::Widget,
  };

  use super::{
    UiOptions,
    WorkspaceDrawerItem,
    WorkspaceDrawerRender,
    display_symbol,
    layout,
    render_screen,
  };
  use crate::{
    lua::AnchorOptions,
    terminal_emulator::TerminalEmulator,
  };

  #[test]
  fn empty_terminal_cells_render_as_spaces() {
    assert_eq!(display_symbol(""), " ");
    assert_eq!(display_symbol("x"), "x");
  }

  #[test]
  fn anchors_reserve_rows_and_resize_the_terminal_workspace() {
    let options = UiOptions {
      top: AnchorOptions {
        height: 2,
        visible: true,
      },
      bottom: AnchorOptions {
        height: 3,
        visible: true,
      },
      ..UiOptions::default()
    };
    let collapsed = layout(Rect::new(0, 0, 100, 30), options, false);
    assert_eq!(collapsed.top_anchor.height, 2);
    assert_eq!(collapsed.bottom_anchor.height, 3);
    assert_eq!(collapsed.drawer.width, 1);
    assert_eq!(collapsed.pane.width, 97);
    assert_eq!(collapsed.pane.height, 23);
    let expanded = layout(Rect::new(0, 0, 100, 30), options, true);
    assert_eq!(expanded.pane, collapsed.pane);
  }

  #[test]
  fn workspace_drawer_collapses_to_initials_and_expands_to_full_names() {
    let area = Rect::new(0, 0, 30, 6);
    let workspaces = [
      WorkspaceDrawerItem {
        id: 1,
        name: "alpha",
      },
      WorkspaceDrawerItem {
        id: 2,
        name: "beta",
      },
    ];
    let collapsed = WorkspaceDrawerRender {
      expanded: false,
      active_id: 2,
      highlighted_id: 2,
      items: &workspaces,
    };
    let mut buffer = Buffer::empty(area);
    (&collapsed).render(area, &mut buffer);
    assert_eq!(buffer[(0, 0)].symbol(), "a");
    assert_eq!(buffer[(0, 1)].symbol(), "b");

    let expanded = WorkspaceDrawerRender {
      expanded: true,
      active_id: 2,
      highlighted_id: 1,
      items: &workspaces,
    };
    (&expanded).render(area, &mut buffer);
    assert_eq!(buffer[(1, 0)].symbol(), "a");
    assert_eq!(buffer[(1, 1)].symbol(), "b");
  }

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
