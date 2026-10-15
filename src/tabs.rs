#[derive(Debug)]
pub struct TabSet<T> {
  tabs: Vec<T>,
  active: usize,
}

impl<T> Default for TabSet<T> {
  fn default() -> Self {
    Self {
      tabs: Vec::new(),
      active: 0,
    }
  }
}

impl<T> TabSet<T> {
  pub fn push(&mut self, tab: T) {
    self.tabs.push(tab);
    self.active = self.tabs.len() - 1;
  }

  pub fn active(&self) -> Option<&T> { self.tabs.get(self.active) }

  pub fn active_mut(&mut self) -> Option<&mut T> { self.tabs.get_mut(self.active) }

  pub fn active_index(&self) -> usize { self.active }

  pub fn len(&self) -> usize { self.tabs.len() }

  pub fn iter(&self) -> impl Iterator<Item = &T> { self.tabs.iter() }

  pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> { self.tabs.iter_mut() }

  pub fn next_item(&mut self) -> bool {
    if self.tabs.len() < 2 {
      return false;
    }
    self.active = (self.active + 1) % self.tabs.len();
    true
  }

  pub fn previous_item(&mut self) -> bool {
    if self.tabs.len() < 2 {
      return false;
    }
    self.active = if self.active == 0 {
      self.tabs.len() - 1
    } else {
      self.active - 1
    };
    true
  }

  pub fn remove_active(&mut self) -> Option<T> {
    if self.tabs.is_empty() {
      return None;
    }
    let removed = self.tabs.remove(self.active);
    if self.active >= self.tabs.len() && !self.tabs.is_empty() {
      self.active = self.tabs.len() - 1;
    }
    Some(removed)
  }

  pub fn is_empty(&self) -> bool { self.tabs.is_empty() }

  pub fn select(&mut self, one_based_index: usize) -> bool {
    if one_based_index == 0 || one_based_index > self.tabs.len() {
      return false;
    }
    let selected = one_based_index - 1;
    if selected == self.active {
      return false;
    }
    self.active = selected;
    true
  }
}

#[cfg(test)]
mod tests {
  use super::TabSet;

  #[test]
  fn navigation_wraps_and_selects_one_based_tab_numbers() {
    let mut tabs = TabSet::default();
    tabs.push("first");
    tabs.push("second");
    tabs.push("third");
    assert_eq!(tabs.active(), Some(&"third"));

    assert!(tabs.next_item());
    assert_eq!(tabs.active(), Some(&"first"));
    assert!(tabs.previous_item());
    assert_eq!(tabs.active(), Some(&"third"));
    assert!(tabs.select(2));
    assert_eq!(tabs.active(), Some(&"second"));
    assert!(!tabs.select(0));
    assert!(!tabs.select(4));
    assert!(!tabs.select(2));
  }

  #[test]
  fn navigation_with_one_tab_is_a_noop() {
    let mut tabs = TabSet::default();
    tabs.push("only");
    assert!(!tabs.next_item());
    assert!(!tabs.previous_item());
    assert_eq!(tabs.active(), Some(&"only"));
  }

  #[test]
  fn closing_active_tab_selects_a_neighbor_and_can_empty_the_set() {
    let mut tabs = TabSet::default();
    tabs.push("first");
    tabs.push("second");
    tabs.push("third");

    assert_eq!(tabs.remove_active(), Some("third"));
    assert_eq!(tabs.active(), Some(&"second"));
    assert_eq!(tabs.remove_active(), Some("second"));
    assert_eq!(tabs.active(), Some(&"first"));
    assert_eq!(tabs.remove_active(), Some("first"));
    assert!(tabs.is_empty());
    assert_eq!(tabs.remove_active(), None);
  }
}
