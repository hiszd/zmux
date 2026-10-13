use crate::{
    lua::{BarSegment, LuaConfig, UiOptions},
    trace::Trace,
    which_key::WhichKeyPopup,
};
use crossterm::cursor::SetCursorStyle;
use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, Widget},
};
use std::time::Instant;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
}

impl CursorShape {
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

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "block" => Ok(Self::Block),
            "underline" => Ok(Self::Underline),
            "bar" => Ok(Self::Bar),
            _ => Err(format!(
                "invalid cursor_shape '{value}'; expected 'block', 'underline', or 'bar'"
            )),
        }
    }
}

#[derive(Default)]
pub struct TerminalRenderer {
    size: (u16, u16),
    previous_cells: Vec<Option<vt100::Cell>>,
    rendered_cells: Vec<RenderedCell>,
    cursor: Option<(bool, u16, u16)>,
}

#[derive(Clone, Default)]
struct RenderedCell {
    symbol: String,
    style: Style,
}

#[derive(Default)]
struct ProjectionStats {
    visited: usize,
    changed: usize,
    nonempty: usize,
    projection_duration: std::time::Duration,
}

#[derive(Clone, Default)]
pub struct BarParts {
    pub left: Option<Vec<BarSegment>>,
    pub center: Option<Vec<BarSegment>>,
    pub right: Option<Vec<BarSegment>>,
}

#[derive(Clone, Default)]
pub struct BarContents {
    pub top: BarParts,
    pub bottom: BarParts,
}

pub struct WorkspaceDrawerItem<'a> {
    pub id: u64,
    pub name: &'a str,
}

pub struct WorkspaceDrawerRender<'a> {
    pub expanded: bool,
    pub active_id: u64,
    pub highlighted_id: u64,
    pub items: &'a [WorkspaceDrawerItem<'a>],
}

pub struct ModalOverlay<'a> {
    pub title: &'a str,
    pub query: Option<&'a str>,
    pub rows: &'a [String],
    pub selected_row: Option<usize>,
    pub footer: &'a str,
}

pub struct UiRender<'a> {
    pub screen: &'a vt100::Screen,
    pub renderer: &'a mut TerminalRenderer,
    pub leader_mode: bool,
    pub tab_count: usize,
    pub active_tab: usize,
    pub active_title: &'a str,
    pub active_exit_code: Option<u32>,
    pub workspace_drawer: WorkspaceDrawerRender<'a>,
    pub modal: Option<ModalOverlay<'a>>,
    pub which_key: WhichKeyPopup,
    pub key_warning: Option<&'a str>,
    pub options: UiOptions,
    pub bars: BarContents,
    pub lua: &'a LuaConfig,
    pub trace: &'a Trace,
    pub force_redraw: &'a mut bool,
}

pub struct UiLayout {
    pub top_anchor: Rect,
    pub bottom_anchor: Rect,
    pub panel: Rect,
    pub drawer: Rect,
    pub pty: Rect,
    pub pane: Rect,
}

impl TerminalRenderer {
    pub fn invalidate(&mut self) {
        self.previous_cells.fill(None);
    }

    fn project_screen(
        &mut self,
        screen: &vt100::Screen,
        width: u16,
        height: u16,
    ) -> ProjectionStats {
        let count = usize::from(width) * usize::from(height);
        if self.size != (width, height) || self.previous_cells.len() != count {
            self.size = (width, height);
            self.previous_cells = vec![None; count];
            self.rendered_cells = vec![RenderedCell::default(); count];
        }

        let started = Instant::now();
        let mut stats = ProjectionStats::default();
        for row in 0..height {
            for column in 0..width {
                let index = usize::from(row) * usize::from(width) + usize::from(column);
                let cell = screen.cell(row, column);
                stats.visited += 1;
                stats.nonempty += usize::from(cell.is_some_and(vt100::Cell::has_contents));
                if cell == self.previous_cells[index].as_ref() {
                    continue;
                }

                stats.changed += 1;
                self.previous_cells[index] = cell.cloned();
                self.rendered_cells[index] = cell.map_or_else(RenderedCell::default, |cell| {
                    let contents = if cell.has_contents() {
                        cell.contents()
                    } else {
                        String::new()
                    };
                    RenderedCell {
                        symbol: display_symbol(&contents).to_owned(),
                        style: cell_style(cell),
                    }
                });
            }
        }
        stats.projection_duration = started.elapsed();
        stats
    }

    fn cursor_state(
        &self,
        screen: &vt100::Screen,
        width: u16,
        height: u16,
    ) -> Option<(bool, u16, u16)> {
        let (row, column) = screen.cursor_position();
        let visible = !screen.hide_cursor() && row < height && column < width;
        Some((visible, row, column))
    }
}

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
    let bottom_anchor = Rect::new(
        area.x,
        area.y + top_height + panel_height,
        area.width,
        bottom_height,
    );
    let drawer_width = drawer_width(panel.width, drawer_expanded);
    let drawer = Rect::new(panel.x, panel.y, drawer_width, panel.height);
    let pty = if drawer_expanded {
        panel
    } else {
        Rect::new(
            panel.x.saturating_add(drawer_width),
            panel.y,
            panel.width.saturating_sub(drawer_width),
            panel.height,
        )
    };
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

pub fn pane_size(width: u16, height: u16, options: UiOptions, drawer_expanded: bool) -> (u16, u16) {
    let layout = layout(Rect::new(0, 0, width, height), options, drawer_expanded);
    (layout.pane.height, layout.pane.width)
}

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

pub fn draw(frame: &mut Frame<'_>, view: &mut UiRender<'_>) -> mlua::Result<()> {
    let frame_started = Instant::now();
    let force_redraw = *view.force_redraw;
    if force_redraw {
        view.renderer.invalidate();
        *view.force_redraw = false;
    }
    let area = frame.area();
    let layout = layout(area, view.options, view.workspace_drawer.expanded);
    let rect_detail = |rect: Option<Rect>| {
        rect.map_or_else(
            || "none".to_owned(),
            |rect| {
                format!(
                    "x={} y={} w={} h={}",
                    rect.x, rect.y, rect.width, rect.height
                )
            },
        )
    };
    view.trace.mark(&format!(
        "ui-frame-layout area={}x{} drawer_expanded={} force_redraw={} pty={} pane={}",
        area.width,
        area.height,
        view.workspace_drawer.expanded,
        force_redraw,
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
        default_bottom_parts(
            view.active_title,
            view.active_exit_code,
            area.width,
            area.height,
        ),
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
    let border_started = Instant::now();
    let screen = view.screen;
    let stats = view
        .renderer
        .project_screen(screen, layout.pane.width, layout.pane.height);
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
        for row in 0..layout.pane.height {
            for column in 0..layout.pane.width {
                let index = usize::from(row) * usize::from(layout.pane.width) + usize::from(column);
                let cell = &view.renderer.rendered_cells[index];
                buffer[(layout.pane.x + column, layout.pane.y + row)]
                    .set_symbol(&cell.symbol)
                    .set_style(cell.style);
            }
        }
        (&view.workspace_drawer).render(layout.drawer, buffer);
        if let Some(modal) = &view.modal {
            modal.render(layout.panel, buffer);
        }
    }
    view.trace.duration(
        "ui-pty-border-render",
        border_started.elapsed(),
        &format!("area={}x{}", layout.pty.width, layout.pty.height),
    );
    let buffer_duration = buffer_started.elapsed();
    let cursor = view
        .renderer
        .cursor_state(screen, layout.pane.width, layout.pane.height);
    let cursor_changed = cursor != view.renderer.cursor;
    view.renderer.cursor = cursor;
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
        "ui-terminal-cell-project",
        stats.projection_duration,
        &format!(
            "visited={} dirty={} reused={} nonempty={}",
            stats.visited,
            stats.changed,
            stats.visited - stats.changed,
            stats.nonempty
        ),
    );
    view.trace.duration(
        "ui-buffer-populate",
        buffer_duration,
        &format!("cells={}", stats.visited),
    );
    view.trace.mark(if cursor_changed {
        "ui-cursor-changed"
    } else {
        "ui-cursor-unchanged"
    });
    view.trace.duration(
        "ui-frame-closure",
        frame_started.elapsed(),
        &format!("area={}x{}", area.width, area.height),
    );

    Ok(())
}

impl Widget for &ModalOverlay<'_> {
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
            .saturating_add(4)
            .min(usize::from(area.width)) as u16;
        let desired_height = modal.rows.len() + usize::from(modal.query.is_some()) + 3;
        let height = desired_height.max(3).min(usize::from(area.height)) as u16;
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
            draw_segments(
                buffer,
                inner,
                y,
                inner.x,
                &[BarSegment {
                    text: format!("> {query}"),
                    role: None,
                    style: Some(Style::default().fg(Color::Yellow).bg(Color::Black)),
                }],
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
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White).bg(Color::Black)
            };
            draw_segments(
                buffer,
                inner,
                y,
                inner.x,
                &[BarSegment {
                    text: text.clone(),
                    role: None,
                    style: Some(style),
                }],
            );
            y += 1;
        }
        if y < bottom {
            draw_segments(
                buffer,
                inner,
                bottom - 1,
                inner.x,
                &[BarSegment {
                    text: modal.footer.to_owned(),
                    role: None,
                    style: Some(Style::default().fg(Color::Gray).bg(Color::Black)),
                }],
            );
        }
    }
}

impl Widget for &WorkspaceDrawerRender<'_> {
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
            draw_segments(
                buffer,
                inner,
                y,
                inner.x,
                &[BarSegment {
                    text,
                    role: None,
                    style: Some(style),
                }],
            );
        }
    }
}

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

fn segments_width(segments: &[BarSegment]) -> u16 {
    segments
        .iter()
        .map(|segment| {
            UnicodeWidthStr::width(segment.text.as_str()).min(usize::from(u16::MAX)) as u16
        })
        .fold(0_u16, u16::saturating_add)
}

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

fn display_symbol(contents: &str) -> &str {
    if contents.is_empty() { " " } else { contents }
}

fn cell_style(cell: &vt100::Cell) -> Style {
    let mut modifiers = Modifier::empty();
    if cell.bold() {
        modifiers.insert(Modifier::BOLD);
    }
    if cell.italic() {
        modifiers.insert(Modifier::ITALIC);
    }
    if cell.underline() {
        modifiers.insert(Modifier::UNDERLINED);
    }
    if cell.inverse() {
        modifiers.insert(Modifier::REVERSED);
    }
    Style::default()
        .fg(color(cell.fgcolor()))
        .bg(color(cell.bgcolor()))
        .add_modifier(modifiers)
}

fn color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(value) => Color::Indexed(value),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

#[allow(dead_code)]
fn _area(_: Rect) {}

#[cfg(test)]
mod tests {
    use super::{
        TerminalRenderer, UiOptions, WorkspaceDrawerItem, WorkspaceDrawerRender, display_symbol,
        layout,
    };
    use crate::lua::AnchorOptions;
    use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};
    use vt100::Parser;

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
        };
        let layout = layout(Rect::new(0, 0, 100, 30), options, false);
        assert_eq!(layout.top_anchor.height, 2);
        assert_eq!(layout.bottom_anchor.height, 3);
        assert_eq!(layout.drawer.width, 1);
        assert_eq!(layout.pane.width, 97);
        assert_eq!(layout.pane.height, 23);
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
    fn renderer_reuses_unchanged_cells_and_detects_erasure() {
        let mut parser = Parser::new(2, 4, 0);
        let mut renderer = TerminalRenderer::default();
        parser.process(b"x");

        let first = renderer.project_screen(parser.screen(), 4, 2);
        assert!(first.changed > 0);
        let unchanged = renderer.project_screen(parser.screen(), 4, 2);
        assert_eq!(unchanged.changed, 0);
        assert_eq!(unchanged.visited, 8);

        parser.process(b"\x1b[1;1H\x1b[K");
        let erased = renderer.project_screen(parser.screen(), 4, 2);
        assert!(erased.changed > 0);
        assert_eq!(renderer.rendered_cells[0].symbol, " ");
    }
}
