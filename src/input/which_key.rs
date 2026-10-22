use ratatui::{
  layout::Rect,
  style::{
    Color,
    Modifier,
    Style,
  },
  text::{
    Line,
    Span,
  },
  widgets::{
    Block,
    Borders,
    Clear,
    Paragraph,
    Widget,
  },
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::key_notation::{
  self,
  KeySeq,
  KeyStroke,
};
use crate::lua::WhichKeyOptions;
const HEADER_ROWS: u16 = 1;
const FOOTER_ROWS: u16 = 1;
const BORDER_ROWS: u16 = 2;
const COLUMN_SPACING: u16 = 3;
const KEY_COLUMN_WIDTH: u16 = 14;
#[derive(Clone, Default)]
pub struct WhichKeyPopup {
  pub prefix: KeySeq,
  pub completions: Vec<(KeyStroke, Option<String>, bool)>,
  pub scroll_offset: usize,
  pub options: WhichKeyOptions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WhichKeyLayout {
  pub outer: Rect,
  pub inner: Rect,
  pub list: Rect,
  pub footer: Option<Rect>,
  pub total_rows: usize,
  pub columns: usize,
}

impl WhichKeyPopup {
  pub fn is_visible(&self) -> bool { !self.completions.is_empty() }

  pub fn layout(&self, viewport: Rect) -> Option<WhichKeyLayout> {
    if !self.is_visible() {
      return None;
    }
    let max_height = self.options.max_height.rows(viewport.height).max(1);
    let maximum_list_rows = max_height.saturating_sub(BORDER_ROWS + HEADER_ROWS).max(1);
    let required_columns = self
      .completions
      .len()
      .div_ceil(usize::from(maximum_list_rows));
    let columns = required_columns.min(self.options.max_columns.max(1));
    let total_rows = completion_row_count(self.completions.len(), columns);
    let scrollable = required_columns > self.options.max_columns.max(1);
    let list_rows = if scrollable {
      maximum_list_rows.saturating_sub(FOOTER_ROWS).max(1)
    } else {
      total_rows as u16
    };
    let footer_rows = if scrollable { FOOTER_ROWS } else { 0 };
    let height = (BORDER_ROWS + HEADER_ROWS + footer_rows + list_rows).min(viewport.height);
    let outer = popup_area(viewport, height);
    let inner = Block::default().borders(Borders::ALL).inner(outer);
    let list = Rect::new(inner.x, inner.y + HEADER_ROWS, inner.width, list_rows.min(inner.height));
    let footer = scrollable.then(|| {
      Rect::new(
        inner.x,
        inner.y + inner.height.saturating_sub(FOOTER_ROWS),
        inner.width,
        FOOTER_ROWS.min(inner.height),
      )
    });
    Some(WhichKeyLayout {
      outer,
      inner,
      list,
      footer,
      total_rows,
      columns,
    })
  }

  pub fn popup_area(&self, viewport: Rect) -> Option<Rect> {
    self.layout(viewport).map(|layout| layout.outer)
  }

  pub fn max_scroll(&self, viewport: Rect) -> usize {
    self.layout(viewport).map_or(0, |layout| {
      layout
        .total_rows
        .saturating_sub(usize::from(layout.list.height))
    })
  }

  pub fn can_scroll(&self, viewport: Rect) -> bool { self.max_scroll(viewport) > 0 }
}

impl Widget for &WhichKeyPopup {
  fn render(self, viewport: Rect, buffer: &mut ratatui::buffer::Buffer) {
    let Some(layout) = self.layout(viewport) else {
      return;
    };
    Clear.render(layout.outer, buffer);
    let block = Block::default()
      .borders(Borders::ALL)
      .title(" Which Key ")
      .border_style(Style::default().fg(Color::Cyan))
      .style(Style::default().bg(Color::Black));
    block.render(layout.outer, buffer);

    let header = Paragraph::new(Line::from(vec![
      Span::styled("Keys after ", Style::default().fg(Color::Gray)),
      Span::styled(
        key_notation::key_seq_to_string(&self.prefix),
        Style::default()
          .fg(Color::Cyan)
          .add_modifier(Modifier::BOLD),
      ),
    ]));
    header
      .render(Rect::new(layout.inner.x, layout.inner.y, layout.inner.width, HEADER_ROWS), buffer);

    let lines = completion_lines(layout.list.width, &self.completions, layout.columns);
    let line_count = lines.len();
    let max_scroll = line_count.saturating_sub(usize::from(layout.list.height));
    let scroll_offset = self.scroll_offset.min(max_scroll) as u16;
    Paragraph::new(lines)
      .scroll((scroll_offset, 0))
      .render(layout.list, buffer);

    if let Some(footer) = layout.footer {
      Paragraph::new(Line::from(vec![Span::styled(
        format!(
          " [ / ] scroll  {}-{} of {} ",
          self.scroll_offset.min(max_scroll) + 1,
          (self.scroll_offset.min(max_scroll) + usize::from(layout.list.height)).min(line_count),
          line_count,
        ),
        Style::default().fg(Color::Gray),
      )]))
      .render(footer, buffer);
    }
  }
}

fn completion_row_count(item_count: usize, columns: usize) -> usize { item_count.div_ceil(columns) }

fn completion_lines(
  width: u16,
  completions: &[(KeyStroke, Option<String>, bool)],
  columns: usize,
) -> Vec<Line<'static>> {
  let columns = columns.max(1).min(completions.len().max(1));
  let rows = completion_row_count(completions.len(), columns);
  let spacing = usize::from(COLUMN_SPACING);
  let column_width =
    usize::from(width.saturating_sub(COLUMN_SPACING * (columns as u16 - 1))) / columns;
  let mut lines = Vec::with_capacity(rows);

  for row in 0..rows {
    let mut spans = Vec::new();
    for column in 0..columns {
      let index = column * rows + row;
      if column > 0 {
        spans.push(Span::raw(" ".repeat(spacing)));
      }
      if let Some((key, description, has_children)) = completions.get(index) {
        spans.extend(completion_spans(key, description.as_deref(), *has_children, column_width));
      } else {
        spans.push(Span::raw(" ".repeat(column_width)));
      }
    }
    lines.push(Line::from(spans));
  }
  lines
}

fn completion_spans(
  key: &KeyStroke,
  description: Option<&str>,
  has_children: bool,
  column_width: usize,
) -> Vec<Span<'static>> {
  let key_column_width = column_width.min(usize::from(KEY_COLUMN_WIDTH));
  let key_text = format!(" {} ", key_notation::key_stroke_to_string(key));
  let key_width = UnicodeWidthStr::width(key_text.as_str());
  let key_text = if key_width > key_column_width {
    truncate_with_ellipsis(&key_text, key_column_width)
  } else {
    key_text
  };
  let key_width = UnicodeWidthStr::width(key_text.as_str());
  let description_width = column_width.saturating_sub(key_column_width);
  let description = match (description, has_children) {
    (Some(description), true) => format!("{description} …"),
    (Some(description), false) => description.to_owned(),
    (None, true) => "…".to_owned(),
    (None, false) => String::new(),
  };
  let description = truncate_with_ellipsis(&description, description_width);
  let description_width_used = UnicodeWidthStr::width(description.as_str());
  let padding = column_width.saturating_sub(key_column_width + description_width_used);

  vec![
    Span::styled(
      format!("{key_text}{}", " ".repeat(key_column_width.saturating_sub(key_width))),
      Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD),
    ),
    Span::styled(
      format!("{description}{}", " ".repeat(padding)),
      Style::default().fg(Color::White),
    ),
  ]
}

fn truncate_with_ellipsis(value: &str, width: usize) -> String {
  if UnicodeWidthStr::width(value) <= width {
    return value.to_owned();
  }
  if width == 0 {
    return String::new();
  }
  let target = width.saturating_sub(UnicodeWidthStr::width("…"));
  let mut result = String::new();
  let mut used = 0;
  for grapheme in value.graphemes(true) {
    let grapheme_width = UnicodeWidthStr::width(grapheme);
    if used + grapheme_width > target {
      break;
    }
    result.push_str(grapheme);
    used += grapheme_width;
  }
  result.push('…');
  result
}

fn popup_area(area: Rect, height: u16) -> Rect {
  Rect::new(
    area.x,
    area.y + area.height.saturating_sub(height),
    area.width,
    height.min(area.height),
  )
}

#[cfg(test)]
mod tests {
  use super::*;

  // Developer approved: do not remove.
  #[test]
  fn popup_is_full_width_and_anchored_to_bottom() {
    let view = Rect::new(3, 4, 80, 24);
    let popup = WhichKeyPopup {
      completions: vec![completion("a"); 5],
      ..WhichKeyPopup::default()
    };
    let shown = popup.layout(view).expect("visible popup").outer;
    assert_eq!(shown.x, view.x);
    assert_eq!(shown.width, view.width);
    assert_eq!(shown.right(), view.right());
    assert_eq!(shown.y + shown.height, view.y + view.height);
    assert_eq!(shown.height, 8);
  }

  // Developer approved: do not remove.
  #[test]
  fn popup_caps_height_and_reports_scroll_range() {
    let view = Rect::new(0, 0, 80, 20);
    let popup = WhichKeyPopup {
      completions: vec![completion("a"); 100],
      ..WhichKeyPopup::default()
    };
    let layout = popup.layout(view).expect("visible popup");
    assert_eq!(layout.outer.height, 11);
    assert!(popup.can_scroll(view));
    assert_eq!(popup.max_scroll(view), 8);
  }

  // Developer approved: do not remove.
  #[test]
  fn column_count_is_capped_to_preserve_description_width() {
    let view = Rect::new(0, 0, 240, 20);
    let popup = WhichKeyPopup {
      completions: vec![completion("a"); 100],
      ..WhichKeyPopup::default()
    };
    let layout = popup.layout(view).expect("visible popup");
    assert_eq!(layout.columns, 7);
    assert_eq!(layout.total_rows, 15);
  }

  // Developer approved: do not remove.
  #[test]
  fn truncation_respects_narrow_widths_and_unicode_graphemes() {
    let cases = [
      ("zero-width", "abcdef", 0, ""),
      ("one-column", "abcdef", 1, "…"),
      ("two-column", "abcdef", 2, "a…"),
      ("ASCII", "abcdefgh", 6, "abcde…"),
      ("narrow CJK", "日本語", 1, "…"),
      ("CJK", "日本語", 5, "日本…"),
      ("combining mark", "e\u{301}clair", 2, "e\u{301}…"),
      ("emoji ZWJ sequence", "👩‍💻work", 3, "👩‍💻…"),
      ("exact fit", "abc", 3, "abc"),
    ];

    for (case, input, width, expected) in cases {
      let actual = truncate_with_ellipsis(input, width);
      assert_eq!(actual, expected, "{case} truncation at width {width}");
      assert!(
        UnicodeWidthStr::width(actual.as_str()) <= width,
        "{case} output exceeds width {width}: {actual:?}"
      );
    }
  }

  fn completion(key: &str) -> (KeyStroke, Option<String>, bool) {
    (
      key_notation::parse_key_notation(key).expect("valid key")[0],
      Some("Description".to_owned()),
      false,
    )
  }
}
