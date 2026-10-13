use crate::key_notation::{self, KeySeq, KeyStroke};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap},
};

#[derive(Clone, Default)]
pub struct WhichKeyPopup {
    pub prefix: KeySeq,
    pub completions: Vec<(KeyStroke, Option<String>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WhichKeyLayout {
    pub outer: Rect,
    pub inner: Rect,
}

impl WhichKeyPopup {
    pub fn is_visible(&self) -> bool {
        !self.completions.is_empty()
    }

    pub fn layout(&self, viewport: Rect) -> Option<WhichKeyLayout> {
        if !self.is_visible() {
            return None;
        }
        let outer = popup_area(
            viewport,
            visible_item_count(viewport, self.completions.len()),
        );
        let block = Block::default().borders(Borders::ALL);
        Some(WhichKeyLayout {
            outer,
            inner: block.inner(outer),
        })
    }

    pub fn popup_area(&self, viewport: Rect) -> Option<Rect> {
        self.layout(viewport).map(|layout| layout.outer)
    }
}

impl Widget for &WhichKeyPopup {
    fn render(self, viewport: Rect, buffer: &mut ratatui::buffer::Buffer) {
        let Some(layout) = self.layout(viewport) else {
            return;
        };
        let popup_area = layout.outer;
        let visible_items = visible_item_count(viewport, self.completions.len());
        Clear.render(layout.outer, buffer);
        let mut lines = vec![Line::from(vec![
            Span::styled("Keys after ", Style::default().fg(Color::Gray)),
            Span::styled(
                key_notation::key_seq_to_string(&self.prefix),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ])];
        for (key, description) in self.completions.iter().take(visible_items) {
            let mut spans = vec![Span::styled(
                format!(" {:<12} ", key_notation::key_stroke_to_string(key)),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )];
            if let Some(description) = description {
                spans.push(Span::styled(
                    description.clone(),
                    Style::default().fg(Color::White),
                ));
            }
            lines.push(Line::from(spans));
        }
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Which Key ")
                    .border_style(Style::default().fg(Color::Cyan))
                    .style(Style::default().bg(Color::Black)),
            )
            .wrap(Wrap { trim: true })
            .render(popup_area, buffer);
    }
}

fn visible_item_count(area: Rect, item_count: usize) -> usize {
    item_count.min(usize::from(area.height.saturating_sub(2)))
}

fn popup_area(area: Rect, visible_items: usize) -> Rect {
    const HORIZONTAL_BORDER_WIDTH: u16 = 2;
    const VERTICAL_BORDER_HEIGHT: usize = 2;

    // This is the outer widget rectangle. Account for both horizontal border
    // cells explicitly, then put them back into the outer width. This keeps
    // the left and right borders part of the same rectangle passed to Clear
    // and Block.
    let content_width = area.width.saturating_sub(HORIZONTAL_BORDER_WIDTH);
    let outer_width = content_width
        .saturating_add(HORIZONTAL_BORDER_WIDTH)
        .min(area.width);
    let height = visible_items.saturating_add(VERTICAL_BORDER_HEIGHT) as u16;
    Rect::new(
        area.x,
        area.y + area.height.saturating_sub(height),
        outer_width,
        height.min(area.height),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_is_full_width_and_anchored_to_bottom() {
        let view = Rect::new(3, 4, 80, 24);
        let shown = popup_area(view, visible_item_count(view, 5));
        assert_eq!(shown.x, view.x);
        assert_eq!(shown.width, view.width);
        assert_eq!(shown.right(), view.right());
        assert_eq!(shown.width.saturating_sub(2), view.width.saturating_sub(2));
        assert_eq!(shown.y + shown.height, view.y + view.height);
        assert_eq!(shown.height, 7);
    }

    #[test]
    fn popup_clips_entries_to_fit_view_height() {
        let view = Rect::new(0, 0, 80, 5);
        let count = visible_item_count(view, 20);
        let shown = popup_area(view, count);
        assert_eq!(count, 3);
        assert_eq!(shown.height, view.height);
    }
}
