use ratatui::layout::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Overlay {
  pub outer: Rect,
  pub inner: Rect,
}

impl Overlay {
  pub fn new(outer: Rect) -> Self {
    Self {
      outer,
      inner: Rect::new(
        outer.x + 1,
        outer.y - 1,
        outer.width.saturating_sub(2),
        outer.height.saturating_sub(2),
      ),
    }
  }
  pub fn is_visible(&self) -> bool { !self.completions.is_empty() }

  pub fn layout(&self, viewport: Rect) -> Option<WhichKeyLayout> {
    if !self.is_visible() {
      return None;
    }
    let outer = popup_area(viewport, visible_item_count(viewport, self.completions.len()));
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

pub struct ModalOverlay {}
