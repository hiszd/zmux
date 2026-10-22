use std::collections::{
  HashMap,
  HashSet,
};

use ratatui::{
  buffer::Buffer,
  layout::Rect,
  style::Style,
  widgets::{
    Block,
    BorderType,
    Borders,
    Clear,
    Widget,
  },
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChildSize {
  Auto,
  Fixed(u16),
  Fill,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChildMeasure {
  pub size: ChildSize,
  pub intrinsic: u16,
}

/// Allocates a row or column's main axis without exceeding its available size.
/// Fixed and auto children keep their requested size until space runs out;
/// fill children share the remaining cells evenly, with remainder cells going
/// to earlier fill children.
pub(crate) fn allocate_main_axis(available: u16, children: &[ChildMeasure]) -> Vec<u16> {
  let mut sizes = vec![0; children.len()];
  let mut remaining = available;
  let fill_count = children
    .iter()
    .filter(|child| child.size == ChildSize::Fill)
    .count();

  for (index, child) in children.iter().enumerate() {
    let requested = match child.size {
      ChildSize::Auto => child.intrinsic,
      ChildSize::Fixed(size) => size,
      ChildSize::Fill => continue,
    };
    let allocated = requested.min(remaining);
    sizes[index] = allocated;
    remaining -= allocated;
  }

  let remaining = usize::from(remaining);
  if let Some(base) = remaining.checked_div(fill_count) {
    let remainder = remaining - base * fill_count;
    let mut fill_index = 0;
    for (index, child) in children.iter().enumerate() {
      if child.size == ChildSize::Fill {
        sizes[index] = (base + usize::from(fill_index < remainder)) as u16;
        fill_index += 1;
      }
    }
  }

  sizes
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LayoutAxis {
  Horizontal,
  Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DrawerSide {
  Left,
  Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DrawerWidth {
  Auto,
  Fixed(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DrawerBorder {
  None,
  Plain,
  Rounded,
  Double,
  Thick,
  LightDoubleDashed,
  HeavyDoubleDashed,
  LightTripleDashed,
  HeavyTripleDashed,
  LightQuadrupleDashed,
  HeavyQuadrupleDashed,
  QuadrantInside,
  QuadrantOutside,
}

impl DrawerBorder {
  fn ratatui_type(self) -> BorderType {
    match self {
      Self::None | Self::Plain => BorderType::Plain,
      Self::Rounded => BorderType::Rounded,
      Self::Double => BorderType::Double,
      Self::Thick => BorderType::Thick,
      Self::LightDoubleDashed => BorderType::LightDoubleDashed,
      Self::HeavyDoubleDashed => BorderType::HeavyDoubleDashed,
      Self::LightTripleDashed => BorderType::LightTripleDashed,
      Self::HeavyTripleDashed => BorderType::HeavyTripleDashed,
      Self::LightQuadrupleDashed => BorderType::LightQuadrupleDashed,
      Self::HeavyQuadrupleDashed => BorderType::HeavyQuadrupleDashed,
      Self::QuadrantInside => BorderType::QuadrantInside,
      Self::QuadrantOutside => BorderType::QuadrantOutside,
    }
  }

  fn borders(self) -> Borders {
    if self == Self::None {
      Borders::NONE
    } else {
      Borders::ALL
    }
  }

  fn horizontal_inset(self) -> u16 { if self == Self::None { 0 } else { 2 } }
}

#[derive(Clone, Debug)]
pub(crate) struct UiChild {
  pub node_id: u64,
  pub size: ChildSize,
}

#[derive(Clone, Debug)]
pub(crate) enum UiNodeKind {
  Text {
    text: String,
    style: Style,
  },
  Layout {
    axis: LayoutAxis,
    children: Vec<UiChild>,
  },
}

#[derive(Clone, Debug)]
pub(crate) struct UiNode {
  pub kind: UiNodeKind,
  parent: Option<u64>,
  drawer_root: Option<u64>,
}

pub(crate) struct UiDrawerOptions {
  pub side: DrawerSide,
  pub width: DrawerWidth,
  pub title: String,
  pub border: DrawerBorder,
  pub border_style: Style,
  pub content_id: u64,
  pub visible: bool,
}

#[derive(Clone)]
pub(crate) struct UiDrawer {
  pub id: u64,
  pub side: DrawerSide,
  pub width: DrawerWidth,
  pub title: String,
  pub border: DrawerBorder,
  pub border_style: Style,
  pub content_id: u64,
  pub visible: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct IntrinsicSize {
  pub width: u16,
  pub height: u16,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum DrawerProperty {
  Side,
  Width,
  Title,
  Border,
  BorderStyle,
}

pub(crate) struct UiTree {
  next_id: u64,
  nodes: HashMap<u64, UiNode>,
  drawers: Vec<UiDrawer>,
  focused_drawer: Option<u64>,
  intrinsic_cache: HashMap<u64, IntrinsicSize>,
  drawer_rects: HashMap<u64, Rect>,
  node_rects: HashMap<u64, Rect>,
  layout_area: Option<Rect>,
  layout_dirty: bool,
}

impl Default for UiTree {
  fn default() -> Self {
    Self {
      next_id: 1,
      nodes: HashMap::new(),
      drawers: Vec::new(),
      focused_drawer: None,
      intrinsic_cache: HashMap::new(),
      drawer_rects: HashMap::new(),
      node_rects: HashMap::new(),
      layout_area: None,
      layout_dirty: true,
    }
  }
}

impl UiTree {
  fn next_id(&mut self) -> Result<u64, String> {
    let id = self.next_id;
    self.next_id = self
      .next_id
      .checked_add(1)
      .ok_or_else(|| "UI identifier space exhausted".to_owned())?;
    Ok(id)
  }

  pub(crate) fn create_text(&mut self, text: String, style: Style) -> Result<u64, String> {
    let id = self.next_id()?;
    self.nodes.insert(id, UiNode {
      kind: UiNodeKind::Text { text, style },
      parent: None,
      drawer_root: None,
    });
    self.layout_dirty = true;
    Ok(id)
  }

  pub(crate) fn create_layout(
    &mut self,
    axis: LayoutAxis,
    children: Vec<UiChild>,
  ) -> Result<u64, String> {
    let id = self.next_id()?;
    self.validate_children(id, &children)?;
    for child in &children {
      self
        .nodes
        .get_mut(&child.node_id)
        .ok_or_else(|| "UI child handle is no longer valid".to_owned())?
        .parent = Some(id);
    }
    self.nodes.insert(id, UiNode {
      kind: UiNodeKind::Layout { axis, children },
      parent: None,
      drawer_root: None,
    });
    self.layout_dirty = true;
    Ok(id)
  }

  pub(crate) fn create_drawer(&mut self, options: UiDrawerOptions) -> Result<u64, String> {
    let id = self.next_id()?;
    let content = self
      .nodes
      .get_mut(&options.content_id)
      .ok_or_else(|| "drawer content handle is no longer valid".to_owned())?;
    if content.parent.is_some() || content.drawer_root.is_some() {
      return Err("a UI node can only belong to one parent".to_owned());
    }
    content.drawer_root = Some(id);
    self.drawers.push(UiDrawer {
      id,
      side: options.side,
      width: options.width,
      title: options.title,
      border: options.border,
      border_style: options.border_style,
      content_id: options.content_id,
      visible: options.visible,
    });
    self.layout_dirty = true;
    Ok(id)
  }

  fn validate_children(&self, parent_id: u64, children: &[UiChild]) -> Result<(), String> {
    let mut unique = HashSet::with_capacity(children.len());
    for child in children {
      if !unique.insert(child.node_id) {
        return Err("a Layout cannot contain the same child more than once".to_owned());
      }
      let node = self
        .nodes
        .get(&child.node_id)
        .ok_or_else(|| "UI child handle is no longer valid".to_owned())?;
      if node.parent.is_some_and(|parent| parent != parent_id) || node.drawer_root.is_some() {
        return Err("a UI node can only belong to one parent".to_owned());
      }
      let mut ancestor = Some(parent_id);
      while let Some(ancestor_id) = ancestor {
        if ancestor_id == child.node_id {
          return Err("a Layout cannot contain one of its ancestors".to_owned());
        }
        ancestor = self
          .nodes
          .get(&ancestor_id)
          .and_then(|ancestor| ancestor.parent);
      }
    }
    Ok(())
  }

  pub(crate) fn set_text(&mut self, id: u64, text: String) -> Result<bool, String> {
    let node = self
      .nodes
      .get_mut(&id)
      .ok_or_else(|| "UI text handle is no longer valid".to_owned())?;
    let UiNodeKind::Text { text: current, .. } = &mut node.kind else {
      return Err("text can only be set on a Text node".to_owned());
    };
    if *current == text {
      return Ok(false);
    }
    *current = text;
    self.invalidate_node(id);
    Ok(true)
  }

  pub(crate) fn set_text_style(&mut self, id: u64, style: Style) -> Result<bool, String> {
    let node = self
      .nodes
      .get_mut(&id)
      .ok_or_else(|| "UI text handle is no longer valid".to_owned())?;
    let UiNodeKind::Text { style: current, .. } = &mut node.kind else {
      return Err("style can only be set on a Text node".to_owned());
    };
    if *current == style {
      return Ok(false);
    }
    *current = style;
    self.layout_dirty = true;
    Ok(true)
  }

  pub(crate) fn set_children(
    &mut self,
    parent_id: u64,
    children: Vec<UiChild>,
  ) -> Result<bool, String> {
    self.validate_children(parent_id, &children)?;
    let old_children: Vec<u64> = match &self
      .nodes
      .get(&parent_id)
      .ok_or_else(|| "Layout handle is no longer valid".to_owned())?
      .kind
    {
      UiNodeKind::Layout { children, .. } => children.iter().map(|child| child.node_id).collect(),
      UiNodeKind::Text { .. } => {
        return Err("children can only be set on a Layout node".to_owned());
      }
    };
    let new_children: HashSet<_> = children.iter().map(|child| child.node_id).collect();
    if old_children.iter().copied().collect::<HashSet<_>>() == new_children
      && old_children.len() == children.len()
    {
      let current = match &self.nodes[&parent_id].kind {
        UiNodeKind::Layout { children, .. } => children,
        UiNodeKind::Text { .. } => unreachable!(),
      };
      if current
        .iter()
        .zip(&children)
        .all(|(left, right)| left.node_id == right.node_id && left.size == right.size)
      {
        return Ok(false);
      }
    }

    for old_id in old_children {
      if !new_children.contains(&old_id)
        && let Some(node) = self.nodes.get_mut(&old_id)
      {
        node.parent = None;
      }
    }
    for child in &children {
      self
        .nodes
        .get_mut(&child.node_id)
        .ok_or_else(|| "UI child handle is no longer valid".to_owned())?
        .parent = Some(parent_id);
    }
    let node = self
      .nodes
      .get_mut(&parent_id)
      .ok_or_else(|| "Layout handle is no longer valid".to_owned())?;
    let UiNodeKind::Layout {
      children: current, ..
    } = &mut node.kind
    else {
      return Err("children can only be set on a Layout node".to_owned());
    };
    *current = children;
    self.invalidate_node(parent_id);
    Ok(true)
  }

  fn invalidate_node(&mut self, id: u64) {
    let mut current = Some(id);
    while let Some(node_id) = current {
      self.intrinsic_cache.remove(&node_id);
      current = self.nodes.get(&node_id).and_then(|node| node.parent);
    }
    self.layout_dirty = true;
  }

  pub(crate) fn set_drawer_property(
    &mut self,
    id: u64,
    property: DrawerProperty,
    value: DrawerValue,
  ) -> Result<bool, String> {
    let drawer = self
      .drawers
      .iter_mut()
      .find(|drawer| drawer.id == id)
      .ok_or_else(|| "Drawer handle is no longer valid".to_owned())?;
    let changed = match (property, value) {
      (DrawerProperty::Side, DrawerValue::Side(value)) if drawer.side != value => {
        drawer.side = value;
        true
      }
      (DrawerProperty::Width, DrawerValue::Width(value)) if drawer.width != value => {
        drawer.width = value;
        true
      }
      (DrawerProperty::Title, DrawerValue::Title(value)) if drawer.title != value => {
        drawer.title = value;
        true
      }
      (DrawerProperty::Border, DrawerValue::Border(value)) if drawer.border != value => {
        drawer.border = value;
        true
      }
      (DrawerProperty::BorderStyle, DrawerValue::Style(value)) if drawer.border_style != value => {
        drawer.border_style = value;
        true
      }
      (DrawerProperty::Side, DrawerValue::Side(_))
      | (DrawerProperty::Width, DrawerValue::Width(_))
      | (DrawerProperty::Title, DrawerValue::Title(_))
      | (DrawerProperty::Border, DrawerValue::Border(_))
      | (DrawerProperty::BorderStyle, DrawerValue::Style(_)) => false,
      _ => return Err("UI property received a value of the wrong type".to_owned()),
    };
    if changed {
      self.layout_dirty = true;
    }
    Ok(changed)
  }

  pub(crate) fn show(&mut self, id: u64) -> Result<bool, String> {
    let drawer = self.drawer_mut(id)?;
    if drawer.visible {
      return Ok(false);
    }
    drawer.visible = true;
    self.layout_dirty = true;
    Ok(true)
  }

  pub(crate) fn hide(&mut self, id: u64) -> Result<bool, String> {
    let drawer = self.drawer_mut(id)?;
    if !drawer.visible {
      return Ok(false);
    }
    drawer.visible = false;
    if self.focused_drawer == Some(id) {
      self.focused_drawer = None;
    }
    self.layout_dirty = true;
    Ok(true)
  }

  pub(crate) fn toggle(&mut self, id: u64) -> Result<bool, String> {
    if self.drawer(id)?.visible {
      self.hide(id)
    } else {
      self.show(id)
    }
  }

  pub(crate) fn focus(&mut self, id: u64) -> Result<bool, String> {
    self.show(id)?;
    if self.focused_drawer == Some(id) {
      return Ok(false);
    }
    self.focused_drawer = Some(id);
    Ok(true)
  }

  pub(crate) fn blur(&mut self, id: u64) -> Result<bool, String> {
    self.drawer(id)?;
    if self.focused_drawer != Some(id) {
      return Ok(false);
    }
    self.focused_drawer = None;
    Ok(true)
  }

  pub(crate) fn focused_drawer(&self) -> Option<u64> { self.focused_drawer }

  pub(crate) fn blur_focused(&mut self) -> bool { self.focused_drawer.take().is_some() }

  fn drawer(&self, id: u64) -> Result<&UiDrawer, String> {
    self
      .drawers
      .iter()
      .find(|drawer| drawer.id == id)
      .ok_or_else(|| "Drawer handle is no longer valid".to_owned())
  }

  fn drawer_mut(&mut self, id: u64) -> Result<&mut UiDrawer, String> {
    self
      .drawers
      .iter_mut()
      .find(|drawer| drawer.id == id)
      .ok_or_else(|| "Drawer handle is no longer valid".to_owned())
  }

  fn intrinsic_size(&mut self, id: u64) -> Result<IntrinsicSize, String> {
    if let Some(size) = self.intrinsic_cache.get(&id) {
      return Ok(*size);
    }
    let kind = self
      .nodes
      .get(&id)
      .ok_or_else(|| "UI node handle is no longer valid".to_owned())?
      .kind
      .clone();
    let size = match kind {
      UiNodeKind::Text { text, .. } => measure_text(&text),
      UiNodeKind::Layout { axis, children } => {
        let mut child_sizes = Vec::with_capacity(children.len());
        for child in &children {
          child_sizes.push((child.size, self.intrinsic_size(child.node_id)?));
        }
        match axis {
          LayoutAxis::Horizontal => IntrinsicSize {
            width: child_sizes
              .iter()
              .map(|(constraint, size)| match constraint {
                ChildSize::Fixed(fixed) => *fixed,
                ChildSize::Auto | ChildSize::Fill => size.width,
              })
              .fold(0, u16::saturating_add),
            height: child_sizes
              .iter()
              .map(|(_, size)| size.height)
              .max()
              .unwrap_or_default(),
          },
          LayoutAxis::Vertical => IntrinsicSize {
            width: child_sizes
              .iter()
              .map(|(_, size)| size.width)
              .max()
              .unwrap_or_default(),
            height: child_sizes
              .iter()
              .map(|(constraint, size)| match constraint {
                ChildSize::Fixed(fixed) => *fixed,
                ChildSize::Auto | ChildSize::Fill => size.height,
              })
              .fold(0, u16::saturating_add),
          },
        }
      }
    };
    self.intrinsic_cache.insert(id, size);
    Ok(size)
  }

  pub(crate) fn render(&mut self, area: Rect, buffer: &mut Buffer) {
    if self.layout_dirty || self.layout_area != Some(area) {
      self.rebuild_layout(area);
    }
    for drawer in &self.drawers {
      if !drawer.visible {
        continue;
      }
      let Some(drawer_area) = self.drawer_rects.get(&drawer.id).copied() else {
        continue;
      };
      if drawer_area.is_empty() {
        continue;
      }
      Clear.render(drawer_area, buffer);
      let block = Block::default()
        .borders(drawer.border.borders())
        .border_type(drawer.border.ratatui_type())
        .border_style(drawer.border_style)
        .title(drawer.title.as_str());
      let content_area = block.inner(drawer_area);
      block.render(drawer_area, buffer);
      self.render_node(drawer.content_id, content_area, buffer);
    }
  }

  fn rebuild_layout(&mut self, area: Rect) {
    self.drawer_rects.clear();
    self.node_rects.clear();
    let mut left_offset = 0_u16;
    let mut right_offset = 0_u16;
    let visible: Vec<(u64, DrawerSide, DrawerWidth, u64, DrawerBorder, String)> = self
      .drawers
      .iter()
      .filter(|drawer| drawer.visible)
      .map(|drawer| {
        (
          drawer.id,
          drawer.side,
          drawer.width,
          drawer.content_id,
          drawer.border,
          drawer.title.clone(),
        )
      })
      .collect();

    for (id, side, width, content_id, border, title) in visible {
      let intrinsic = self.intrinsic_size(content_id).unwrap_or_default();
      let border_width = border.horizontal_inset();
      let desired = match width {
        DrawerWidth::Auto => intrinsic
          .width
          .max(UnicodeWidthStr::width(title.as_str()).min(usize::from(u16::MAX)) as u16)
          .saturating_add(border_width),
        DrawerWidth::Fixed(width) => width,
      };
      let available = area
        .width
        .saturating_sub(left_offset.saturating_add(right_offset));
      let offset = match side {
        DrawerSide::Left => left_offset,
        DrawerSide::Right => right_offset,
      };
      let width = desired.min(available);
      let x = match side {
        DrawerSide::Left => area.x.saturating_add(offset),
        DrawerSide::Right => area
          .x
          .saturating_add(area.width.saturating_sub(offset).saturating_sub(width)),
      };
      let drawer_area = Rect::new(x, area.y, width, area.height);
      match side {
        DrawerSide::Left => left_offset = left_offset.saturating_add(width),
        DrawerSide::Right => right_offset = right_offset.saturating_add(width),
      }
      self.drawer_rects.insert(id, drawer_area);
      let inner = if border == DrawerBorder::None {
        drawer_area
      } else {
        Block::default().borders(Borders::ALL).inner(drawer_area)
      };
      self.layout_node(content_id, inner);
    }
    self.layout_area = Some(area);
    self.layout_dirty = false;
  }

  fn layout_node(&mut self, id: u64, area: Rect) {
    self.node_rects.insert(id, area);
    let Some(node) = self.nodes.get(&id).map(|node| node.kind.clone()) else {
      return;
    };
    let UiNodeKind::Layout { axis, children } = node else {
      return;
    };
    let mut measures = Vec::with_capacity(children.len());
    for child in &children {
      let intrinsic = self.intrinsic_size(child.node_id).unwrap_or_default();
      measures.push(ChildMeasure {
        size: child.size,
        intrinsic: match axis {
          LayoutAxis::Horizontal => intrinsic.width,
          LayoutAxis::Vertical => intrinsic.height,
        },
      });
    }
    let available = match axis {
      LayoutAxis::Horizontal => area.width,
      LayoutAxis::Vertical => area.height,
    };
    let sizes = allocate_main_axis(available, &measures);
    let mut offset = 0_u16;
    for (child, size) in children.into_iter().zip(sizes) {
      let child_area = match axis {
        LayoutAxis::Horizontal => {
          Rect::new(area.x.saturating_add(offset), area.y, size, area.height)
        }
        LayoutAxis::Vertical => Rect::new(area.x, area.y.saturating_add(offset), area.width, size),
      };
      offset = offset.saturating_add(size);
      self.layout_node(child.node_id, child_area);
    }
  }

  fn render_node(&self, id: u64, area: Rect, buffer: &mut Buffer) {
    if area.is_empty() {
      return;
    }
    let Some(node) = self.nodes.get(&id) else {
      return;
    };
    match &node.kind {
      UiNodeKind::Text { text, style } => draw_text(buffer, area, text, *style),
      UiNodeKind::Layout { children, .. } => {
        for child in children {
          if let Some(child_area) = self.node_rects.get(&child.node_id).copied() {
            self.render_node(child.node_id, child_area, buffer);
          }
        }
      }
    }
  }
}

#[derive(Clone, Debug)]
pub(crate) enum DrawerValue {
  Side(DrawerSide),
  Width(DrawerWidth),
  Title(String),
  Border(DrawerBorder),
  Style(Style),
}

fn measure_text(text: &str) -> IntrinsicSize {
  let mut width = 0_usize;
  let mut height = 0_usize;
  for line in text.split('\n') {
    width = width.max(UnicodeWidthStr::width(line));
    height += 1;
  }
  IntrinsicSize {
    width: width.min(usize::from(u16::MAX)) as u16,
    height: height.min(usize::from(u16::MAX)) as u16,
  }
}

fn draw_text(buffer: &mut Buffer, area: Rect, text: &str, style: Style) {
  for (row, line) in text.split('\n').enumerate().take(usize::from(area.height)) {
    let y = area.y.saturating_add(row as u16);
    let mut x = area.x;
    let end = area.x.saturating_add(area.width);
    for grapheme in line.graphemes(true) {
      let width = UnicodeWidthStr::width(grapheme) as u16;
      if width == 0 {
        if x > area.x {
          let previous = buffer[(x - 1, y)].symbol().to_owned();
          buffer[(x - 1, y)].set_symbol(&format!("{previous}{grapheme}"));
        }
        continue;
      }
      if x.saturating_add(width) > end {
        break;
      }
      buffer[(x, y)].set_symbol(grapheme).set_style(style);
      x = x.saturating_add(width);
    }
  }
}

#[cfg(test)]
mod tests {
  use super::{
    ChildMeasure,
    ChildSize,
    allocate_main_axis,
  };

  #[test]
  fn fixed_auto_and_fill_children_allocate_space_in_order() {
    let children = [
      ChildMeasure {
        size: ChildSize::Fixed(2),
        intrinsic: 0,
      },
      ChildMeasure {
        size: ChildSize::Auto,
        intrinsic: 3,
      },
      ChildMeasure {
        size: ChildSize::Fill,
        intrinsic: 0,
      },
      ChildMeasure {
        size: ChildSize::Fill,
        intrinsic: 0,
      },
    ];

    assert_eq!(allocate_main_axis(13, &children), vec![2, 3, 4, 4]);
  }

  #[test]
  fn overflowing_fixed_and_auto_children_are_clipped_before_fill_space() {
    let children = [
      ChildMeasure {
        size: ChildSize::Fixed(6),
        intrinsic: 0,
      },
      ChildMeasure {
        size: ChildSize::Auto,
        intrinsic: 4,
      },
      ChildMeasure {
        size: ChildSize::Fill,
        intrinsic: 0,
      },
    ];

    assert_eq!(allocate_main_axis(7, &children), vec![6, 1, 0]);
  }
}
