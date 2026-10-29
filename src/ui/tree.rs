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

use super::{
  StyleRegistry,
  StyleSource,
};

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
pub(crate) enum BarPosition {
  Top,
  Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HorizontalAlignment {
  Left,
  Center,
  Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UiSlice {
  pub index: u16,
  pub slice_width: u16,
  pub node_id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UiSize {
  Auto,
  Fixed(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToastPosition {
  TopRight,
  TopCenter,
  BottomRight,
  BottomCenter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UiElementType {
  Bar {
    position: BarPosition,
  },
  Drawer {
    side: DrawerSide,
    width: UiSize,
  },
  Modal {
    width: UiSize,
    height: UiSize,
  },
  Popover {
    x: u16,
    y: u16,
    width: UiSize,
    height: UiSize,
  },
  BottomPanel {
    height: UiSize,
  },
  Toast {
    position: ToastPosition,
    duration: std::time::Duration,
  },
}

impl UiElementType {
  fn auto_focus(self) -> bool { matches!(self, Self::Modal { .. } | Self::Popover { .. }) }

  fn captures_input(self) -> bool { matches!(self, Self::Modal { .. } | Self::Popover { .. }) }

  fn can_focus(self) -> bool { !matches!(self, Self::Bar { .. } | Self::Toast { .. }) }
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
  pub(crate) fn ratatui_type(self) -> BorderType {
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

  pub(crate) fn borders(self) -> Borders {
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
    style: StyleSource,
  },
  Layout {
    axis: LayoutAxis,
    children: Vec<UiChild>,
  },
  Slices {
    slices: u16,
    items: Vec<UiSlice>,
  },
  Align {
    child_id: u64,
    horizontal: HorizontalAlignment,
  },
}

#[derive(Clone, Debug)]
pub(crate) struct UiNode {
  pub kind: UiNodeKind,
  parent: Option<u64>,
  element_root: Option<u64>,
  lua_handle_alive: bool,
}

pub(crate) struct UiElementOptions {
  pub element_type: UiElementType,
  pub title: String,
  pub border: DrawerBorder,
  pub border_style: StyleSource,
  pub style: StyleSource,
  pub content_id: u64,
  pub visible: bool,
}

#[derive(Clone)]
pub(crate) struct UiElement {
  pub id: u64,
  pub element_type: UiElementType,
  pub title: String,
  pub border: DrawerBorder,
  pub border_style: StyleSource,
  pub style: StyleSource,
  pub content_id: u64,
  pub visible: bool,
  previous_focus: Option<u64>,
  toast_deadline: Option<std::time::Instant>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct IntrinsicSize {
  pub width: u16,
  pub height: u16,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum UiElementProperty {
  Side,
  Width,
  Height,
  X,
  Y,
  Position,
  Duration,
  Title,
  Border,
  BorderStyle,
  Style,
}

pub(crate) struct UiTree {
  next_id: u64,
  nodes: HashMap<u64, UiNode>,
  elements: Vec<UiElement>,
  focused_element: Option<u64>,
  intrinsic_cache: HashMap<u64, IntrinsicSize>,
  element_rects: HashMap<u64, Rect>,
  node_rects: HashMap<u64, Rect>,
  layout_area: Option<Rect>,
  layout_dirty: bool,
}

impl Default for UiTree {
  fn default() -> Self {
    Self {
      next_id: 1,
      nodes: HashMap::new(),
      elements: Vec::new(),
      focused_element: None,
      intrinsic_cache: HashMap::new(),
      element_rects: HashMap::new(),
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

  pub(crate) fn create_text(&mut self, text: String, style: StyleSource) -> Result<u64, String> {
    let id = self.next_id()?;
    self.nodes.insert(id, UiNode {
      kind: UiNodeKind::Text { text, style },
      parent: None,
      element_root: None,
      lua_handle_alive: true,
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
      element_root: None,
      lua_handle_alive: true,
    });
    self.layout_dirty = true;
    Ok(id)
  }

  pub(crate) fn create_slices(&mut self, slices: u16, items: Vec<UiSlice>) -> Result<u64, String> {
    let id = self.next_id()?;
    self.validate_slices(id, slices, &items)?;
    for item in &items {
      self
        .nodes
        .get_mut(&item.node_id)
        .ok_or_else(|| "UI slice item handle is no longer valid".to_owned())?
        .parent = Some(id);
    }
    self.nodes.insert(id, UiNode {
      kind: UiNodeKind::Slices { slices, items },
      parent: None,
      element_root: None,
      lua_handle_alive: true,
    });
    self.layout_dirty = true;
    Ok(id)
  }

  pub(crate) fn create_align(
    &mut self,
    child_id: u64,
    horizontal: HorizontalAlignment,
  ) -> Result<u64, String> {
    let id = self.next_id()?;
    self.validate_children(id, &[UiChild {
      node_id: child_id,
      size: ChildSize::Auto,
    }])?;
    self
      .nodes
      .get_mut(&child_id)
      .ok_or_else(|| "UI alignment child handle is no longer valid".to_owned())?
      .parent = Some(id);
    self.nodes.insert(id, UiNode {
      kind: UiNodeKind::Align {
        child_id,
        horizontal,
      },
      parent: None,
      element_root: None,
      lua_handle_alive: true,
    });
    self.layout_dirty = true;
    Ok(id)
  }

  pub(crate) fn create_element(&mut self, options: UiElementOptions) -> Result<u64, String> {
    if let UiElementType::Bar { position } = options.element_type {
      let count = self
        .elements
        .iter()
        .filter(|element| matches!(element.element_type, UiElementType::Bar { position: existing } if existing == position))
        .count();
      if count >= 2 {
        let edge = match position {
          BarPosition::Top => "top",
          BarPosition::Bottom => "bottom",
        };
        return Err(format!("at most two bars may be created at the {edge} edge"));
      }
      if self.intrinsic_size(options.content_id)?.height > 1 {
        return Err("Bar content must fit in one row".to_owned());
      }
    }
    let id = self.next_id()?;
    let content = self
      .nodes
      .get_mut(&options.content_id)
      .ok_or_else(|| "UI element content handle is no longer valid".to_owned())?;
    if content.parent.is_some() || content.element_root.is_some() {
      return Err("a UI node can only belong to one parent".to_owned());
    }
    content.element_root = Some(id);
    let toast_deadline = match options.element_type {
      UiElementType::Toast { duration, .. } if options.visible => {
        Some(std::time::Instant::now() + duration)
      }
      _ => None,
    };
    let previous_focus = if options.visible && options.element_type.auto_focus() {
      self.focused_element
    } else {
      None
    };
    self.elements.push(UiElement {
      id,
      element_type: options.element_type,
      title: options.title,
      border: options.border,
      border_style: options.border_style,
      style: options.style,
      content_id: options.content_id,
      visible: options.visible,
      previous_focus,
      toast_deadline,
    });
    if options.visible && options.element_type.auto_focus() {
      self.focused_element = Some(id);
    }
    self.layout_dirty = true;
    Ok(id)
  }

  pub(crate) fn release_node_handle(&mut self, id: u64) -> bool {
    let Some(node) = self.nodes.get_mut(&id) else {
      return false;
    };
    if !node.lua_handle_alive {
      return false;
    }
    node.lua_handle_alive = false;
    self.prune_unowned_node(id)
  }

  pub(crate) fn remove_element(&mut self, id: u64) -> bool {
    let Some(index) = self.elements.iter().position(|element| element.id == id) else {
      return false;
    };
    let element = self.elements.remove(index);
    let fallback_focus = element.previous_focus.filter(|focused_id| {
      self
        .elements
        .iter()
        .any(|candidate| candidate.id == *focused_id && candidate.visible)
    });
    for candidate in &mut self.elements {
      if candidate.previous_focus == Some(id) {
        candidate.previous_focus = fallback_focus;
      }
    }
    if self.focused_element == Some(id) {
      self.focused_element = fallback_focus;
    }
    self.element_rects.remove(&id);
    if let Some(content) = self.nodes.get_mut(&element.content_id)
      && content.element_root == Some(id)
    {
      content.element_root = None;
    }
    self.layout_dirty = true;
    self.prune_unowned_node(element.content_id);
    true
  }

  fn prune_unowned_node(&mut self, id: u64) -> bool {
    let is_unowned = self.nodes.get(&id).is_some_and(|node| {
      !node.lua_handle_alive && node.parent.is_none() && node.element_root.is_none()
    });
    if !is_unowned {
      return false;
    }
    let Some(node) = self.nodes.remove(&id) else {
      return false;
    };
    self.intrinsic_cache.remove(&id);
    self.node_rects.remove(&id);
    let children = match node.kind {
      UiNodeKind::Layout { children, .. } => children
        .into_iter()
        .map(|child| child.node_id)
        .collect::<Vec<_>>(),
      UiNodeKind::Slices { items, .. } => items
        .into_iter()
        .map(|item| item.node_id)
        .collect::<Vec<_>>(),
      UiNodeKind::Align { child_id, .. } => vec![child_id],
      UiNodeKind::Text { .. } => Vec::new(),
    };
    for child_id in children {
      if let Some(node) = self.nodes.get_mut(&child_id)
        && node.parent == Some(id)
      {
        node.parent = None;
      }
      self.prune_unowned_node(child_id);
    }
    self.layout_dirty = true;
    true
  }

  fn validate_slices(&self, parent_id: u64, slices: u16, items: &[UiSlice]) -> Result<(), String> {
    if slices == 0 {
      return Err("slice layouts require at least one slice".to_owned());
    }
    let mut occupied = vec![false; usize::from(slices)];
    let mut children = Vec::with_capacity(items.len());
    for item in items {
      if item.index == 0 || item.slice_width == 0 {
        return Err("slice index and slice_width must be positive".to_owned());
      }
      let start = usize::from(item.index - 1);
      let end = start + usize::from(item.slice_width);
      if end > occupied.len() {
        return Err("slice item extends beyond the configured slice count".to_owned());
      }
      if occupied[start..end].iter().any(|cell| *cell) {
        return Err("slice items cannot overlap".to_owned());
      }
      occupied[start..end].fill(true);
      children.push(UiChild {
        node_id: item.node_id,
        size: ChildSize::Auto,
      });
    }
    self.validate_children(parent_id, &children)
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
      if node.parent.is_some_and(|parent| parent != parent_id) || node.element_root.is_some() {
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
    let current = match &self
      .nodes
      .get(&id)
      .ok_or_else(|| "UI text handle is no longer valid".to_owned())?
      .kind
    {
      UiNodeKind::Text { text, .. } => text.clone(),
      _ => return Err("text can only be set on a Text node".to_owned()),
    };
    if current == text {
      return Ok(false);
    }
    let node = self
      .nodes
      .get_mut(&id)
      .ok_or_else(|| "UI text handle is no longer valid".to_owned())?;
    let UiNodeKind::Text { text: stored, .. } = &mut node.kind else {
      return Err("text can only be set on a Text node".to_owned());
    };
    *stored = text;
    self.invalidate_node(id);
    if let Err(error) = self.validate_bar_ancestor(id) {
      if let Some(node) = self.nodes.get_mut(&id)
        && let UiNodeKind::Text { text, .. } = &mut node.kind
      {
        *text = current;
      }
      self.invalidate_node(id);
      return Err(error);
    }
    Ok(true)
  }

  pub(crate) fn set_text_style(&mut self, id: u64, style: StyleSource) -> Result<bool, String> {
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
    let old_layout_children = match &self
      .nodes
      .get(&parent_id)
      .ok_or_else(|| "Layout handle is no longer valid".to_owned())?
      .kind
    {
      UiNodeKind::Layout { children, .. } => children.clone(),
      _ => return Err("children can only be set on a row or column Layout node".to_owned()),
    };
    let old_children: Vec<u64> = old_layout_children
      .iter()
      .map(|child| child.node_id)
      .collect();
    let new_children: HashSet<_> = children.iter().map(|child| child.node_id).collect();
    if old_children.iter().copied().collect::<HashSet<_>>() == new_children
      && old_children.len() == children.len()
      && old_layout_children
        .iter()
        .zip(&children)
        .all(|(left, right)| left.node_id == right.node_id && left.size == right.size)
    {
      return Ok(false);
    }

    let detached: Vec<_> = old_children
      .iter()
      .copied()
      .filter(|old_id| !new_children.contains(old_id))
      .collect();
    for old_id in &detached {
      if let Some(node) = self.nodes.get_mut(old_id) {
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
    if let Err(error) = self.validate_bar_ancestor(parent_id) {
      let new_ids: Vec<_> = self
        .nodes
        .get(&parent_id)
        .and_then(|node| match &node.kind {
          UiNodeKind::Layout { children, .. } => {
            Some(children.iter().map(|child| child.node_id).collect())
          }
          _ => None,
        })
        .unwrap_or_default();
      for child_id in new_ids {
        if !old_children.contains(&child_id)
          && let Some(node) = self.nodes.get_mut(&child_id)
        {
          node.parent = None;
        }
      }
      for child_id in &old_children {
        if let Some(node) = self.nodes.get_mut(child_id) {
          node.parent = Some(parent_id);
        }
      }
      if let Some(node) = self.nodes.get_mut(&parent_id)
        && let UiNodeKind::Layout { children, .. } = &mut node.kind
      {
        *children = old_layout_children;
      }
      self.invalidate_node(parent_id);
      return Err(error);
    }
    for detached_id in detached {
      self.prune_unowned_node(detached_id);
    }
    Ok(true)
  }

  pub(crate) fn set_slices(
    &mut self,
    id: u64,
    slices: u16,
    items: Vec<UiSlice>,
  ) -> Result<bool, String> {
    self.validate_slices(id, slices, &items)?;
    let (old_slices, old_items) = match &self
      .nodes
      .get(&id)
      .ok_or_else(|| "slice Layout handle is no longer valid".to_owned())?
      .kind
    {
      UiNodeKind::Slices { slices, items } => (*slices, items.clone()),
      _ => return Err("slice properties can only be set on a Slices node".to_owned()),
    };
    if old_slices == slices && old_items == items {
      return Ok(false);
    }
    let old_ids: Vec<_> = old_items.iter().map(|item| item.node_id).collect();
    let new_ids: HashSet<_> = items.iter().map(|item| item.node_id).collect();
    for old_id in old_ids.iter().filter(|old_id| !new_ids.contains(old_id)) {
      if let Some(node) = self.nodes.get_mut(old_id) {
        node.parent = None;
      }
    }
    for item in &items {
      self
        .nodes
        .get_mut(&item.node_id)
        .ok_or_else(|| "UI slice item handle is no longer valid".to_owned())?
        .parent = Some(id);
    }
    let node = self
      .nodes
      .get_mut(&id)
      .ok_or_else(|| "slice Layout handle is no longer valid".to_owned())?;
    let UiNodeKind::Slices {
      slices: current_slices,
      items: current_items,
    } = &mut node.kind
    else {
      return Err("slice properties can only be set on a Slices node".to_owned());
    };
    *current_slices = slices;
    *current_items = items;
    self.invalidate_node(id);
    if let Err(error) = self.validate_bar_ancestor(id) {
      let new_ids: Vec<_> = self
        .nodes
        .get(&id)
        .and_then(|node| match &node.kind {
          UiNodeKind::Slices { items, .. } => Some(items.iter().map(|item| item.node_id).collect()),
          _ => None,
        })
        .unwrap_or_default();
      for child_id in new_ids {
        if !old_ids.contains(&child_id)
          && let Some(node) = self.nodes.get_mut(&child_id)
        {
          node.parent = None;
        }
      }
      for child_id in &old_ids {
        if let Some(node) = self.nodes.get_mut(child_id) {
          node.parent = Some(id);
        }
      }
      if let Some(node) = self.nodes.get_mut(&id)
        && let UiNodeKind::Slices {
          slices: current_slices,
          items: current_items,
        } = &mut node.kind
      {
        *current_slices = old_slices;
        *current_items = old_items;
      }
      self.invalidate_node(id);
      return Err(error);
    }
    for old_id in old_ids.iter().filter(|old_id| !new_ids.contains(old_id)) {
      self.prune_unowned_node(*old_id);
    }
    Ok(true)
  }

  pub(crate) fn set_slice_items(&mut self, id: u64, items: Vec<UiSlice>) -> Result<bool, String> {
    let slices = match &self
      .nodes
      .get(&id)
      .ok_or_else(|| "slice Layout handle is no longer valid".to_owned())?
      .kind
    {
      UiNodeKind::Slices { slices, .. } => *slices,
      _ => return Err("items can only be set on a Slices node".to_owned()),
    };
    self.set_slices(id, slices, items)
  }

  pub(crate) fn set_slice_count(&mut self, id: u64, slices: u16) -> Result<bool, String> {
    let items = match &self
      .nodes
      .get(&id)
      .ok_or_else(|| "slice Layout handle is no longer valid".to_owned())?
      .kind
    {
      UiNodeKind::Slices { items, .. } => items.clone(),
      _ => return Err("slices can only be set on a Slices node".to_owned()),
    };
    self.set_slices(id, slices, items)
  }

  pub(crate) fn set_alignment_child(&mut self, id: u64, child_id: u64) -> Result<bool, String> {
    self.validate_children(id, &[UiChild {
      node_id: child_id,
      size: ChildSize::Auto,
    }])?;
    let old_id = match self
      .nodes
      .get(&id)
      .ok_or_else(|| "alignment handle is no longer valid".to_owned())?
      .kind
    {
      UiNodeKind::Align { child_id, .. } => child_id,
      _ => return Err("content can only be set on an Align node".to_owned()),
    };
    if old_id == child_id {
      return Ok(false);
    }
    self
      .nodes
      .get_mut(&old_id)
      .ok_or_else(|| "UI alignment child handle is no longer valid".to_owned())?
      .parent = None;
    self
      .nodes
      .get_mut(&child_id)
      .ok_or_else(|| "UI alignment child handle is no longer valid".to_owned())?
      .parent = Some(id);
    let node = self
      .nodes
      .get_mut(&id)
      .ok_or_else(|| "alignment handle is no longer valid".to_owned())?;
    let UiNodeKind::Align {
      child_id: current, ..
    } = &mut node.kind
    else {
      return Err("content can only be set on an Align node".to_owned());
    };
    *current = child_id;
    self.invalidate_node(id);
    if let Err(error) = self.validate_bar_ancestor(id) {
      if let Some(node) = self.nodes.get_mut(&child_id) {
        node.parent = None;
      }
      if let Some(node) = self.nodes.get_mut(&old_id) {
        node.parent = Some(id);
      }
      if let Some(node) = self.nodes.get_mut(&id)
        && let UiNodeKind::Align {
          child_id: current, ..
        } = &mut node.kind
      {
        *current = old_id;
      }
      self.invalidate_node(id);
      return Err(error);
    }
    self.prune_unowned_node(old_id);
    Ok(true)
  }

  pub(crate) fn set_alignment(
    &mut self,
    id: u64,
    horizontal: HorizontalAlignment,
  ) -> Result<bool, String> {
    let node = self
      .nodes
      .get_mut(&id)
      .ok_or_else(|| "alignment handle is no longer valid".to_owned())?;
    let UiNodeKind::Align {
      horizontal: current,
      ..
    } = &mut node.kind
    else {
      return Err("horizontal can only be set on an Align node".to_owned());
    };
    if *current == horizontal {
      return Ok(false);
    }
    *current = horizontal;
    self.layout_dirty = true;
    Ok(true)
  }

  pub(crate) fn replace_element_content(
    &mut self,
    id: u64,
    content_id: u64,
  ) -> Result<bool, String> {
    let (is_bar, old_content_id) = {
      let element = self.element(id)?;
      (matches!(element.element_type, UiElementType::Bar { .. }), element.content_id)
    };
    if !is_bar {
      return Err("content can only be replaced on a Bar".to_owned());
    }
    if old_content_id == content_id {
      return Ok(false);
    }
    let new_content = self
      .nodes
      .get(&content_id)
      .ok_or_else(|| "UI element content handle is no longer valid".to_owned())?;
    if new_content.parent.is_some() || new_content.element_root.is_some() {
      return Err("a UI node can only belong to one parent".to_owned());
    }
    if self.intrinsic_size(content_id)?.height > 1 {
      return Err("Bar content must fit in one row".to_owned());
    }
    self
      .nodes
      .get_mut(&old_content_id)
      .ok_or_else(|| "UI element content handle is no longer valid".to_owned())?
      .element_root = None;
    self
      .nodes
      .get_mut(&content_id)
      .ok_or_else(|| "UI element content handle is no longer valid".to_owned())?
      .element_root = Some(id);
    self
      .element_mut(id)
      .ok_or_else(|| "UI element handle is no longer valid".to_owned())?
      .content_id = content_id;
    self.layout_dirty = true;
    self.prune_unowned_node(old_content_id);
    Ok(true)
  }

  pub(crate) fn bar_counts(&self) -> (u16, u16) {
    let mut top = 0;
    let mut bottom = 0;
    for element in self.elements.iter().filter(|element| element.visible) {
      match element.element_type {
        UiElementType::Bar {
          position: BarPosition::Top,
        } => top += 1,
        UiElementType::Bar {
          position: BarPosition::Bottom,
        } => bottom += 1,
        _ => {}
      }
    }
    (top, bottom)
  }

  fn validate_bar_ancestor(&mut self, id: u64) -> Result<(), String> {
    let mut current = Some(id);
    while let Some(node_id) = current {
      let (parent, element_root) = self
        .nodes
        .get(&node_id)
        .map(|node| (node.parent, node.element_root))
        .ok_or_else(|| "UI node handle is no longer valid".to_owned())?;
      if let Some(element_id) = element_root
        && let Some(content_id) = self
          .elements
          .iter()
          .find(|element| element.id == element_id)
          .filter(|element| matches!(element.element_type, UiElementType::Bar { .. }))
          .map(|element| element.content_id)
        && self.intrinsic_size(content_id)?.height > 1
      {
        return Err("Bar content must fit in one row".to_owned());
      }
      current = parent;
    }
    Ok(())
  }

  fn invalidate_node(&mut self, id: u64) {
    let mut current = Some(id);
    while let Some(node_id) = current {
      self.intrinsic_cache.remove(&node_id);
      current = self.nodes.get(&node_id).and_then(|node| node.parent);
    }
    self.layout_dirty = true;
  }

  pub(crate) fn set_element_property(
    &mut self,
    id: u64,
    property: UiElementProperty,
    value: UiElementValue,
  ) -> Result<bool, String> {
    let element = self
      .element_mut(id)
      .ok_or_else(|| "UI element handle is no longer valid".to_owned())?;
    let changed = match (property, value) {
      (UiElementProperty::Side, UiElementValue::Side(value)) => match &mut element.element_type {
        UiElementType::Drawer { side, .. } if *side != value => {
          *side = value;
          true
        }
        UiElementType::Drawer { .. } => false,
        _ => return Err("'side' is only valid for a Drawer".to_owned()),
      },
      (UiElementProperty::Width, UiElementValue::Size(value)) => match &mut element.element_type {
        UiElementType::Drawer { width, .. }
        | UiElementType::Modal { width, .. }
        | UiElementType::Popover { width, .. }
          if *width != value =>
        {
          *width = value;
          true
        }
        UiElementType::Drawer { .. }
        | UiElementType::Modal { .. }
        | UiElementType::Popover { .. } => false,
        _ => return Err("'width' is not valid for this UI element".to_owned()),
      },
      (UiElementProperty::Height, UiElementValue::Size(value)) => match &mut element.element_type {
        UiElementType::Modal { height, .. }
        | UiElementType::Popover { height, .. }
        | UiElementType::BottomPanel { height }
          if *height != value =>
        {
          *height = value;
          true
        }
        UiElementType::Modal { .. }
        | UiElementType::Popover { .. }
        | UiElementType::BottomPanel { .. } => false,
        _ => return Err("'height' is not valid for this UI element".to_owned()),
      },
      (UiElementProperty::X, UiElementValue::Coordinate(value)) => {
        match &mut element.element_type {
          UiElementType::Popover { x, .. } if *x != value => {
            *x = value;
            true
          }
          UiElementType::Popover { .. } => false,
          _ => return Err("'x' is only valid for a Popover".to_owned()),
        }
      }
      (UiElementProperty::Y, UiElementValue::Coordinate(value)) => {
        match &mut element.element_type {
          UiElementType::Popover { y, .. } if *y != value => {
            *y = value;
            true
          }
          UiElementType::Popover { .. } => false,
          _ => return Err("'y' is only valid for a Popover".to_owned()),
        }
      }
      (UiElementProperty::Position, UiElementValue::Position(value)) => {
        match &mut element.element_type {
          UiElementType::Toast { position, .. } if *position != value => {
            *position = value;
            true
          }
          UiElementType::Toast { .. } => false,
          _ => return Err("'position' is only valid for a Toast".to_owned()),
        }
      }
      (UiElementProperty::Duration, UiElementValue::Duration(value)) => {
        match &mut element.element_type {
          UiElementType::Toast { duration, .. } if *duration != value => {
            *duration = value;
            true
          }
          UiElementType::Toast { .. } => false,
          _ => return Err("'duration' is only valid for a Toast".to_owned()),
        }
      }
      (UiElementProperty::Title, UiElementValue::Title(value)) if element.title != value => {
        element.title = value;
        true
      }
      (UiElementProperty::Border, UiElementValue::Border(value)) if element.border != value => {
        element.border = value;
        true
      }
      (UiElementProperty::BorderStyle, UiElementValue::Style(value))
        if element.border_style != value =>
      {
        element.border_style = value;
        true
      }
      (UiElementProperty::Style, UiElementValue::Style(value)) => {
        if !matches!(element.element_type, UiElementType::Bar { .. }) {
          return Err("'style' is only valid for a Bar".to_owned());
        }
        if element.style == value {
          false
        } else {
          element.style = value;
          true
        }
      }
      (UiElementProperty::Title, UiElementValue::Title(_))
      | (UiElementProperty::Border, UiElementValue::Border(_))
      | (UiElementProperty::BorderStyle, UiElementValue::Style(_)) => false,
      _ => return Err("UI property received a value of the wrong type".to_owned()),
    };
    if changed
      && matches!(property, UiElementProperty::Duration)
      && let UiElementType::Toast { duration, .. } = element.element_type
      && element.visible
    {
      element.toast_deadline = Some(std::time::Instant::now() + duration);
    }
    if changed {
      self.layout_dirty = true;
    }
    Ok(changed)
  }

  pub(crate) fn show(&mut self, id: u64) -> Result<bool, String> {
    let prior_focus = self.focused_element;
    let element = self
      .element_mut(id)
      .ok_or_else(|| "UI element handle is no longer valid".to_owned())?;
    let was_visible = element.visible;
    let auto_focus = element.element_type.auto_focus();
    if auto_focus && !was_visible && prior_focus != Some(id) {
      element.previous_focus = prior_focus;
    }
    element.visible = true;
    if let UiElementType::Toast { duration, .. } = element.element_type {
      element.toast_deadline = Some(std::time::Instant::now() + duration);
    }
    if auto_focus {
      self.focused_element = Some(id);
    }
    if !was_visible || auto_focus {
      self.layout_dirty = true;
    }
    Ok(!was_visible || auto_focus)
  }

  pub(crate) fn hide(&mut self, id: u64) -> Result<bool, String> {
    let previous_focus = {
      let element = self
        .element_mut(id)
        .ok_or_else(|| "UI element handle is no longer valid".to_owned())?;
      if !element.visible {
        return Ok(false);
      }
      element.visible = false;
      element.toast_deadline = None;
      element.previous_focus.take()
    };
    if self.focused_element == Some(id) {
      self.focused_element = previous_focus.filter(|focused_id| {
        self
          .elements
          .iter()
          .any(|element| element.id == *focused_id && element.visible)
      });
    }
    self.layout_dirty = true;
    Ok(true)
  }

  pub(crate) fn toggle(&mut self, id: u64) -> Result<bool, String> {
    if self.element(id)?.visible {
      self.hide(id)
    } else {
      self.show(id)
    }
  }

  pub(crate) fn focus(&mut self, id: u64) -> Result<bool, String> {
    if self.focused_element != Some(id) && self.focused_element_captures_input() {
      return Err("hide the active Modal or Popover before focusing another element".to_owned());
    }
    if !self.element(id)?.element_type.can_focus() {
      return Err("Bars and Toast elements cannot receive focus".to_owned());
    }
    let was_focused = self.focused_element == Some(id);
    let shown = self.show(id)?;
    if was_focused || self.focused_element == Some(id) {
      return Ok(shown);
    }
    self.focused_element = Some(id);
    Ok(true)
  }

  pub(crate) fn blur(&mut self, id: u64) -> Result<bool, String> {
    let element = self.element(id)?;
    if element.visible && element.element_type.captures_input() {
      return Err("Modal and Popover elements must be hidden rather than blurred".to_owned());
    }
    if self.focused_element != Some(id) {
      return Ok(false);
    }
    self.focused_element = None;
    Ok(true)
  }

  pub(crate) fn focused_element(&self) -> Option<u64> { self.focused_element }

  pub(crate) fn focused_element_captures_input(&self) -> bool {
    self
      .focused_element
      .and_then(|id| self.element(id).ok())
      .is_some_and(|element| element.element_type.captures_input())
  }

  pub(crate) fn blur_focused(&mut self) -> bool { self.focused_element.take().is_some() }

  pub(crate) fn dismiss_focused_element(&mut self) -> bool {
    self
      .focused_element
      .and_then(|id| self.hide(id).ok())
      .unwrap_or(false)
  }

  pub(crate) fn expire_toasts(&mut self, now: std::time::Instant) -> bool {
    let mut expired = false;
    for element in &mut self.elements {
      if element.visible
        && element
          .toast_deadline
          .is_some_and(|deadline| now >= deadline)
      {
        element.visible = false;
        element.toast_deadline = None;
        if self.focused_element == Some(element.id) {
          self.focused_element = None;
        }
        expired = true;
      }
    }
    if expired {
      self.layout_dirty = true;
    }
    expired
  }

  fn element(&self, id: u64) -> Result<&UiElement, String> {
    self
      .elements
      .iter()
      .find(|element| element.id == id)
      .ok_or_else(|| "UI element handle is no longer valid".to_owned())
  }

  fn element_mut(&mut self, id: u64) -> Option<&mut UiElement> {
    self.elements.iter_mut().find(|element| element.id == id)
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
      UiNodeKind::Slices { items, .. } => {
        let mut size = IntrinsicSize::default();
        for item in items {
          let child = self.intrinsic_size(item.node_id)?;
          size.width = size.width.max(child.width);
          size.height = size.height.max(child.height);
        }
        size
      }
      UiNodeKind::Align { child_id, .. } => self.intrinsic_size(child_id)?,
    };
    self.intrinsic_cache.insert(id, size);
    Ok(size)
  }

  pub(crate) fn render_bars(
    &mut self,
    top: Rect,
    bottom: Rect,
    buffer: &mut Buffer,
    styles: &StyleRegistry,
  ) {
    let bars: Vec<_> = self
      .elements
      .iter()
      .filter_map(|element| {
        if !element.visible {
          return None;
        }
        let position = match element.element_type {
          UiElementType::Bar { position } => position,
          _ => return None,
        };
        Some((element.id, position, element.content_id, element.style.clone()))
      })
      .collect();
    let mut top_index = 0_u16;
    let mut bottom_index = 0_u16;
    for (id, position, content_id, style_source) in bars {
      let style = styles.resolve_source(&style_source);
      let area = match position {
        BarPosition::Top => {
          let y = top.y.saturating_add(top_index);
          top_index = top_index.saturating_add(1);
          (y < top.y.saturating_add(top.height)).then(|| Rect::new(top.x, y, top.width, 1))
        }
        BarPosition::Bottom => {
          let offset = bottom_index;
          bottom_index = bottom_index.saturating_add(1);
          (offset < bottom.height).then(|| {
            Rect::new(
              bottom.x,
              bottom
                .y
                .saturating_add(bottom.height.saturating_sub(1 + offset)),
              bottom.width,
              1,
            )
          })
        }
      };
      let Some(area) = area else {
        continue;
      };
      self.element_rects.insert(id, area);
      self.layout_node(content_id, area);
      for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
          buffer[(x, y)].set_symbol(" ").set_style(style);
        }
      }
      self.render_node(content_id, area, style, styles, buffer);
    }
  }

  pub(crate) fn render(&mut self, area: Rect, buffer: &mut Buffer, styles: &StyleRegistry) {
    if self.layout_dirty || self.layout_area != Some(area) {
      self.rebuild_layout(area);
    }
    for element in &self.elements {
      if !element.visible || matches!(element.element_type, UiElementType::Bar { .. }) {
        continue;
      }
      let Some(element_area) = self.element_rects.get(&element.id).copied() else {
        continue;
      };
      if element_area.is_empty() {
        continue;
      }
      if matches!(element.element_type, UiElementType::Modal { .. }) {
        for y in area.y..area.y.saturating_add(area.height) {
          for x in area.x..area.x.saturating_add(area.width) {
            let style = buffer[(x, y)]
              .style()
              .add_modifier(ratatui::style::Modifier::DIM);
            buffer[(x, y)].set_style(style);
          }
        }
      }
      Clear.render(element_area, buffer);
      let block = Block::default()
        .borders(element.border.borders())
        .border_type(element.border.ratatui_type())
        .border_style(styles.resolve_source(&element.border_style))
        .title(element.title.as_str());
      let content_area = self
        .node_rects
        .get(&element.content_id)
        .copied()
        .unwrap_or_else(|| block.inner(element_area));
      block.render(element_area, buffer);
      self.render_node(element.content_id, content_area, Style::default(), styles, buffer);
    }
  }

  fn rebuild_layout(&mut self, area: Rect) {
    self.element_rects.clear();
    self.node_rects.clear();
    let mut left_offset = 0_u16;
    let mut right_offset = 0_u16;
    let visible: Vec<_> = self
      .elements
      .iter()
      .filter(|element| {
        element.visible && !matches!(element.element_type, UiElementType::Bar { .. })
      })
      .map(|element| {
        (
          element.id,
          element.element_type,
          element.content_id,
          element.border,
          element.title.clone(),
        )
      })
      .collect();

    for (id, element_type, content_id, border, title) in visible {
      let intrinsic = self.intrinsic_size(content_id).unwrap_or_default();
      let border_x = border.horizontal_inset();
      let border_y = border.horizontal_inset();
      let title_width = UnicodeWidthStr::width(title.as_str()).min(usize::from(u16::MAX)) as u16;
      let auto_width = intrinsic.width.max(title_width).saturating_add(border_x);
      let auto_height = intrinsic.height.saturating_add(border_y);
      let element_area = match element_type {
        UiElementType::Bar { .. } => continue,
        UiElementType::Drawer { side, width } => {
          let desired = match width {
            UiSize::Auto => auto_width,
            UiSize::Fixed(width) => width,
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
          match side {
            DrawerSide::Left => left_offset = left_offset.saturating_add(width),
            DrawerSide::Right => right_offset = right_offset.saturating_add(width),
          }
          Rect::new(x, area.y, width, area.height)
        }
        UiElementType::Modal { width, height } => {
          let width = resolve_size(width, auto_width, area.width);
          let height = resolve_size(height, auto_height, area.height);
          Rect::new(
            area.x.saturating_add(area.width.saturating_sub(width) / 2),
            area
              .y
              .saturating_add(area.height.saturating_sub(height) / 2),
            width,
            height,
          )
        }
        UiElementType::Popover {
          x,
          y,
          width,
          height,
        } => {
          let width = resolve_size(width, auto_width, area.width);
          let height = resolve_size(height, auto_height, area.height);
          let x = x.min(area.width.saturating_sub(width));
          let y = y.min(area.height.saturating_sub(height));
          Rect::new(area.x.saturating_add(x), area.y.saturating_add(y), width, height)
        }
        UiElementType::BottomPanel { height } => {
          let height = resolve_size(height, auto_height, area.height);
          Rect::new(
            area.x,
            area.y.saturating_add(area.height.saturating_sub(height)),
            area.width,
            height,
          )
        }
        UiElementType::Toast { position, .. } => {
          let width = auto_width.min(area.width);
          let height = auto_height.min(area.height);
          let x = match position {
            ToastPosition::TopRight | ToastPosition::BottomRight => {
              area.x.saturating_add(area.width.saturating_sub(width))
            }
            ToastPosition::TopCenter | ToastPosition::BottomCenter => {
              area.x.saturating_add(area.width.saturating_sub(width) / 2)
            }
          };
          let y = match position {
            ToastPosition::TopRight | ToastPosition::TopCenter => area.y,
            ToastPosition::BottomRight | ToastPosition::BottomCenter => {
              area.y.saturating_add(area.height.saturating_sub(height))
            }
          };
          Rect::new(x, y, width, height)
        }
      };
      self.element_rects.insert(id, element_area);
      let content_area = if border == DrawerBorder::None {
        element_area
      } else {
        Block::default().borders(Borders::ALL).inner(element_area)
      };
      self.layout_node(content_id, content_area);
    }
    self.layout_area = Some(area);
    self.layout_dirty = false;
  }

  fn layout_node(&mut self, id: u64, area: Rect) {
    self.node_rects.insert(id, area);
    let Some(node) = self.nodes.get(&id).map(|node| node.kind.clone()) else {
      return;
    };
    match node {
      UiNodeKind::Layout { axis, children } => {
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
            LayoutAxis::Vertical => {
              Rect::new(area.x, area.y.saturating_add(offset), area.width, size)
            }
          };
          offset = offset.saturating_add(size);
          self.layout_node(child.node_id, child_area);
        }
      }
      UiNodeKind::Slices { slices, items } => {
        for item in items {
          let start = (u32::from(area.width) * u32::from(item.index - 1)) / u32::from(slices);
          let end = (u32::from(area.width) * u32::from(item.index - 1 + item.slice_width))
            / u32::from(slices);
          let child_area = Rect::new(
            area.x.saturating_add(start as u16),
            area.y,
            end.saturating_sub(start) as u16,
            area.height,
          );
          self.layout_node(item.node_id, child_area);
        }
      }
      UiNodeKind::Align {
        child_id,
        horizontal,
      } => {
        let intrinsic = self.intrinsic_size(child_id).unwrap_or_default();
        let width = intrinsic.width.min(area.width);
        let offset = match horizontal {
          HorizontalAlignment::Left => 0,
          HorizontalAlignment::Center => area.width.saturating_sub(width) / 2,
          HorizontalAlignment::Right => area.width.saturating_sub(width),
        };
        self.layout_node(
          child_id,
          Rect::new(area.x.saturating_add(offset), area.y, width, area.height),
        );
      }
      UiNodeKind::Text { .. } => {}
    }
  }

  fn render_node(
    &self,
    id: u64,
    area: Rect,
    inherited_style: Style,
    styles: &StyleRegistry,
    buffer: &mut Buffer,
  ) {
    if area.is_empty() {
      return;
    }
    let Some(node) = self.nodes.get(&id) else {
      return;
    };
    match &node.kind {
      UiNodeKind::Text { text, style } => {
        draw_text(buffer, area, text, inherited_style.patch(styles.resolve_source(style)))
      }
      UiNodeKind::Layout { children, .. } => {
        for child in children {
          if let Some(child_area) = self.node_rects.get(&child.node_id).copied() {
            self.render_node(child.node_id, child_area, inherited_style, styles, buffer);
          }
        }
      }
      UiNodeKind::Slices { items, .. } => {
        for item in items {
          if let Some(child_area) = self.node_rects.get(&item.node_id).copied() {
            self.render_node(item.node_id, child_area, inherited_style, styles, buffer);
          }
        }
      }
      UiNodeKind::Align { child_id, .. } => {
        if let Some(child_area) = self.node_rects.get(child_id).copied() {
          self.render_node(*child_id, child_area, inherited_style, styles, buffer);
        }
      }
    }
  }
}

#[derive(Clone, Debug)]
pub(crate) enum UiElementValue {
  Side(DrawerSide),
  Size(UiSize),
  Coordinate(u16),
  Position(ToastPosition),
  Duration(std::time::Duration),
  Title(String),
  Border(DrawerBorder),
  Style(StyleSource),
}

fn resolve_size(size: UiSize, intrinsic: u16, available: u16) -> u16 {
  match size {
    UiSize::Auto => intrinsic.min(available),
    UiSize::Fixed(size) => size.min(available),
  }
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
