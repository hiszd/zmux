use std::collections::HashMap;

use ratatui::style::{
  Color,
  Modifier,
  Style,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StyleAttribute<T> {
  Value(T),
  Reference(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StyleDefinition {
  pub fg: Option<StyleAttribute<Color>>,
  pub bg: Option<StyleAttribute<Color>>,
  pub bold: Option<StyleAttribute<bool>>,
  pub italic: Option<StyleAttribute<bool>>,
  pub underline: Option<StyleAttribute<bool>>,
  pub reverse: Option<StyleAttribute<bool>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StyleSource {
  Inline(Style),
  Group(String),
}

impl Default for StyleSource {
  fn default() -> Self { Self::Inline(Style::default()) }
}

/// Per-Client style definitions and named, non-activating theme collections.
/// Theme application patches the active definitions and resolves references
/// before committing, so an invalid update cannot partially affect rendering.
pub(crate) struct StyleRegistry {
  groups: HashMap<String, StyleDefinition>,
  themes: HashMap<String, HashMap<String, StyleDefinition>>,
  resolved: HashMap<String, Style>,
  active_theme: String,
}

impl Default for StyleRegistry {
  fn default() -> Self {
    let groups = default_groups();
    let resolved = resolve_all(&groups).expect("built-in style groups are valid");
    Self {
      themes: HashMap::from([("default".to_owned(), groups.clone())]),
      groups,
      resolved,
      active_theme: "default".to_owned(),
    }
  }
}

impl StyleRegistry {
  pub(crate) fn create_group(
    &mut self,
    name: String,
    definition: StyleDefinition,
  ) -> Result<bool, String> {
    validate_name(&name, "style group")?;
    let mut candidate = self.groups.clone();
    candidate.insert(name, definition);
    let resolved = resolve_all(&candidate)?;
    let changed = self.groups != candidate;
    self.groups = candidate;
    self.resolved = resolved;
    Ok(changed)
  }

  pub(crate) fn define_theme(
    &mut self,
    name: String,
    definitions: HashMap<String, StyleDefinition>,
  ) -> Result<(), String> {
    validate_name(&name, "theme")?;
    let candidate = self.with_theme_patch(&definitions);
    resolve_all(&candidate)?;
    self.themes.insert(name, definitions);
    Ok(())
  }

  pub(crate) fn apply_theme(&mut self, name: &str) -> Result<bool, String> {
    let definitions = self
      .themes
      .get(name)
      .ok_or_else(|| format!("unknown style theme '{name}'"))?;
    let candidate = self.with_theme_patch(definitions);
    let resolved = resolve_all(&candidate)?;
    let changed = self.groups != candidate || self.active_theme != name;
    self.groups = candidate;
    self.resolved = resolved;
    self.active_theme = name.to_owned();
    Ok(changed)
  }

  pub(crate) fn resolve(&self, name: &str) -> Style {
    self.resolved.get(name).copied().unwrap_or_default()
  }

  pub(crate) fn resolve_source(&self, source: &StyleSource) -> Style {
    match source {
      StyleSource::Inline(style) => *style,
      StyleSource::Group(name) => self.resolve(name),
    }
  }

  pub(crate) fn theme_names(&self) -> Vec<String> {
    let mut names: Vec<_> = self.themes.keys().cloned().collect();
    names.sort();
    names
  }

  fn with_theme_patch(
    &self,
    definitions: &HashMap<String, StyleDefinition>,
  ) -> HashMap<String, StyleDefinition> {
    let mut groups = self.groups.clone();
    groups.extend(
      definitions
        .iter()
        .map(|(name, definition)| (name.clone(), definition.clone())),
    );
    groups
  }
}

fn validate_name(name: &str, kind: &str) -> Result<(), String> {
  if name.trim().is_empty() {
    Err(format!("{kind} name must not be empty"))
  } else {
    Ok(())
  }
}

fn resolve_all(
  groups: &HashMap<String, StyleDefinition>,
) -> Result<HashMap<String, Style>, String> {
  groups
    .keys()
    .map(|name| resolve_group(groups, name).map(|style| (name.clone(), style)))
    .collect()
}

fn resolve_group(groups: &HashMap<String, StyleDefinition>, name: &str) -> Result<Style, String> {
  let fg = resolve_attribute(groups, name, "fg", |definition| definition.fg.as_ref())?;
  let bg = resolve_attribute(groups, name, "bg", |definition| definition.bg.as_ref())?;
  let bold = resolve_attribute(groups, name, "bold", |definition| definition.bold.as_ref())?
    .unwrap_or(false);
  let italic = resolve_attribute(groups, name, "italic", |definition| definition.italic.as_ref())?
    .unwrap_or(false);
  let underline =
    resolve_attribute(groups, name, "underline", |definition| definition.underline.as_ref())?
      .unwrap_or(false);
  let reverse =
    resolve_attribute(groups, name, "reverse", |definition| definition.reverse.as_ref())?
      .unwrap_or(false);

  let mut style = Style {
    fg,
    bg,
    ..Style::default()
  };
  style = if bold {
    style.add_modifier(Modifier::BOLD)
  } else {
    style.remove_modifier(Modifier::BOLD)
  };
  style = if italic {
    style.add_modifier(Modifier::ITALIC)
  } else {
    style.remove_modifier(Modifier::ITALIC)
  };
  style = if underline {
    style.add_modifier(Modifier::UNDERLINED)
  } else {
    style.remove_modifier(Modifier::UNDERLINED)
  };
  style = if reverse {
    style.add_modifier(Modifier::REVERSED)
  } else {
    style.remove_modifier(Modifier::REVERSED)
  };
  Ok(style)
}

fn resolve_attribute<T: Copy>(
  groups: &HashMap<String, StyleDefinition>,
  group: &str,
  property: &str,
  get: impl Fn(&StyleDefinition) -> Option<&StyleAttribute<T>> + Copy,
) -> Result<Option<T>, String> {
  let mut path = Vec::new();
  resolve_attribute_inner(groups, group, property, get, &mut path)
}

fn resolve_attribute_inner<T: Copy>(
  groups: &HashMap<String, StyleDefinition>,
  group: &str,
  property: &str,
  get: impl Fn(&StyleDefinition) -> Option<&StyleAttribute<T>> + Copy,
  path: &mut Vec<String>,
) -> Result<Option<T>, String> {
  if path.iter().any(|entry| entry == group) {
    path.push(group.to_owned());
    return Err(format!("cyclic style reference for '{property}': {}", path.join(" -> "),));
  }
  let Some(definition) = groups.get(group) else {
    return Err(format!("unknown style group '{group}' referenced by '{property}'"));
  };
  let Some(attribute) = get(definition) else {
    return Ok(None);
  };
  match attribute {
    StyleAttribute::Value(value) => Ok(Some(*value)),
    StyleAttribute::Reference(target) => {
      path.push(group.to_owned());
      let result = resolve_attribute_inner(groups, target, property, get, path);
      path.pop();
      result
    }
  }
}

fn default_groups() -> HashMap<String, StyleDefinition> {
  let mut groups = HashMap::new();
  let mut insert = |name: &str, definition| {
    groups.insert(name.to_owned(), definition);
  };
  insert("Pty.Border", StyleDefinition::default());
  insert("Bar", style(Some(Color::Gray), Some(Color::Black), false));
  insert("Bar.Tab", style(Some(Color::Gray), None, false));
  insert("Bar.TabActive", style(Some(Color::Black), Some(Color::Cyan), true));
  insert("Bar.Leader", style(Some(Color::Gray), Some(Color::Blue), false));
  insert("Bar.LeaderIndicator", style(Some(Color::Black), Some(Color::Yellow), true));
  insert("Bar.Status", style(Some(Color::Gray), Some(Color::Black), false));
  insert("Bar.Error", style(Some(Color::Red), Some(Color::Black), false));
  insert("Modal.Border", StyleDefinition::default());
  insert("Modal.Background", style(Some(Color::Gray), Some(Color::Black), false));
  insert("Modal.Query", style(Some(Color::Yellow), Some(Color::Black), false));
  insert("Modal.Selected", style(Some(Color::Black), Some(Color::Cyan), true));
  insert("Modal.Item", style(Some(Color::White), Some(Color::Black), false));
  insert("Modal.Footer", style(Some(Color::Gray), Some(Color::Black), false));
  insert("Warning.Key", style(Some(Color::Yellow), None, true));
  insert("Warning.Reload", style(Some(Color::Yellow), None, true));
  insert("WhichKey.Border", style(Some(Color::Cyan), None, false));
  insert("WhichKey.Background", style(None, Some(Color::Black), false));
  insert("WhichKey.Header", style(Some(Color::Gray), None, false));
  insert("WhichKey.Prefix", style(Some(Color::Cyan), None, true));
  insert("WhichKey.Footer", style(Some(Color::Gray), None, false));
  insert("WhichKey.Key", style(Some(Color::Yellow), None, true));
  insert("WhichKey.Description", style(Some(Color::White), None, false));
  groups
}

fn style(fg: Option<Color>, bg: Option<Color>, bold: bool) -> StyleDefinition {
  StyleDefinition {
    fg: fg.map(StyleAttribute::Value),
    bg: bg.map(StyleAttribute::Value),
    bold: Some(StyleAttribute::Value(bold)),
    ..StyleDefinition::default()
  }
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;

  use ratatui::style::{
    Color,
    Modifier,
  };

  use super::{
    StyleAttribute,
    StyleDefinition,
    StyleRegistry,
  };

  #[test]
  fn group_attributes_resolve_literals_references_and_neutral_defaults() {
    let mut styles = StyleRegistry::default();
    styles
      .create_group("base".to_owned(), StyleDefinition {
        fg: Some(StyleAttribute::Value(Color::Rgb(255, 0, 0))),
        bold: Some(StyleAttribute::Value(true)),
        ..StyleDefinition::default()
      })
      .expect("define base group");
    styles
      .create_group("derived".to_owned(), StyleDefinition {
        fg: Some(StyleAttribute::Reference("base".to_owned())),
        bold: Some(StyleAttribute::Reference("base".to_owned())),
        italic: Some(StyleAttribute::Value(false)),
        ..StyleDefinition::default()
      })
      .expect("define derived group");

    let resolved = styles.resolve("derived");
    assert_eq!(resolved.fg, Some(Color::Rgb(255, 0, 0)));
    assert!(resolved.add_modifier.contains(Modifier::BOLD));
    assert!(!resolved.add_modifier.contains(Modifier::ITALIC));
    assert_eq!(resolved.bg, None);
  }

  #[test]
  fn defining_theme_does_not_switch_and_applying_it_patches_only_listed_groups() {
    let mut styles = StyleRegistry::default();
    styles
      .create_group("custom".to_owned(), StyleDefinition {
        fg: Some(StyleAttribute::Value(Color::Green)),
        ..StyleDefinition::default()
      })
      .expect("define custom group");
    let original = styles.resolve("Modal.Selected");
    let theme = HashMap::from([("Modal.Selected".to_owned(), StyleDefinition {
      fg: Some(StyleAttribute::Value(Color::Magenta)),
      ..StyleDefinition::default()
    })]);

    styles
      .define_theme("violet".to_owned(), theme)
      .expect("define theme");
    assert_eq!(styles.resolve("Modal.Selected"), original);
    styles.apply_theme("violet").expect("apply theme");

    assert_eq!(styles.resolve("Modal.Selected").fg, Some(Color::Magenta));
    assert_eq!(styles.resolve("custom").fg, Some(Color::Green));
  }

  #[test]
  fn invalid_theme_definition_does_not_change_active_groups() {
    let mut styles = StyleRegistry::default();
    let before = styles.resolve("Modal.Selected");
    styles
      .define_theme(
        "invalid".to_owned(),
        HashMap::from([
          ("Modal.Selected".to_owned(), StyleDefinition {
            fg: Some(StyleAttribute::Value(Color::Yellow)),
            ..StyleDefinition::default()
          }),
          ("cycle_a".to_owned(), StyleDefinition {
            fg: Some(StyleAttribute::Reference("cycle_b".to_owned())),
            ..StyleDefinition::default()
          }),
          ("cycle_b".to_owned(), StyleDefinition {
            fg: Some(StyleAttribute::Reference("cycle_a".to_owned())),
            ..StyleDefinition::default()
          }),
        ]),
      )
      .expect_err("theme with a cycle must be rejected");
    assert_eq!(styles.resolve("Modal.Selected"), before);
  }
}
