use crossterm::event::{
  KeyCode,
  KeyEvent as CrosstermKeyEvent,
  KeyModifiers,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
  Char(char),
  Enter,
  Escape,
  Space,
  Tab,
  Backspace,
  Delete,
  Insert,
  Home,
  End,
  PageUp,
  PageDown,
  Up,
  Down,
  Left,
  Right,
  F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyStroke {
  pub key: Key,
  pub ctrl: bool,
  pub alt: bool,
  pub shift: bool,
}

pub type KeySeq = Vec<KeyStroke>;

impl KeyStroke {
  pub fn from_crossterm(event: CrosstermKeyEvent) -> Option<Self> {
    let (key, inferred_shift) = match event.code {
      KeyCode::Char(' ') => (Key::Space, false),
      KeyCode::Char(character) => {
        (Key::Char(character.to_ascii_lowercase()), character.is_ascii_uppercase())
      }
      KeyCode::Enter => (Key::Enter, false),
      KeyCode::Esc => (Key::Escape, false),
      KeyCode::Backspace => (Key::Backspace, false),
      KeyCode::Tab => (Key::Tab, false),
      KeyCode::Delete => (Key::Delete, false),
      KeyCode::Insert => (Key::Insert, false),
      KeyCode::Home => (Key::Home, false),
      KeyCode::End => (Key::End, false),
      KeyCode::PageUp => (Key::PageUp, false),
      KeyCode::PageDown => (Key::PageDown, false),
      KeyCode::Left => (Key::Left, false),
      KeyCode::Right => (Key::Right, false),
      KeyCode::Up => (Key::Up, false),
      KeyCode::Down => (Key::Down, false),
      KeyCode::F(number) => (Key::F(number), false),
      _ => return None,
    };
    Some(Self {
      key,
      ctrl: event.modifiers.contains(KeyModifiers::CONTROL),
      alt: event.modifiers.contains(KeyModifiers::ALT),
      shift: inferred_shift || event.modifiers.contains(KeyModifiers::SHIFT),
    })
  }
}

pub fn parse_key_notation(input: &str) -> Result<KeySeq, String> {
  let mut chars = input.chars().peekable();
  let mut sequence = Vec::new();
  while let Some(character) = chars.next() {
    if character == '<' {
      let mut token = String::new();
      let mut closed = false;
      for next in chars.by_ref() {
        if next == '>' {
          closed = true;
          break;
        }
        token.push(next);
      }
      if !closed || token.is_empty() {
        return Err(format!("invalid key notation near '<{token}'"));
      }
      sequence.push(parse_token(&token)?);
    } else {
      sequence.push(KeyStroke {
        key: Key::Char(character.to_ascii_lowercase()),
        ctrl: false,
        alt: false,
        shift: character.is_ascii_uppercase(),
      });
    }
  }
  if sequence.is_empty() {
    return Err("key notation cannot be empty".to_owned());
  }
  Ok(sequence)
}

fn parse_token(token: &str) -> Result<KeyStroke, String> {
  let mut remaining = token;
  let mut ctrl = false;
  let mut alt = false;
  let mut shift = false;
  while let Some((modifier, rest)) = remaining.split_once('-') {
    match modifier.to_ascii_lowercase().as_str() {
      "c" => ctrl = true,
      "m" | "a" => alt = true,
      "s" => shift = true,
      _ => break,
    }
    remaining = rest;
  }
  if remaining.is_empty() {
    return Err(format!("missing key name in <{token}>"));
  }
  let name = remaining;
  let key = match name.to_ascii_lowercase().as_str() {
    "cr" | "enter" | "return" => Key::Enter,
    "esc" | "escape" => Key::Escape,
    "lt" => Key::Char('<'),
    "bar" => Key::Char('|'),
    "bslash" => Key::Char('\\'),
    "space" | "spc" => Key::Space,
    "tab" => Key::Tab,
    "bs" | "backspace" => Key::Backspace,
    "del" | "delete" => Key::Delete,
    "ins" | "insert" => Key::Insert,
    "home" => Key::Home,
    "end" => Key::End,
    "pageup" => Key::PageUp,
    "pagedown" => Key::PageDown,
    "up" => Key::Up,
    "down" => Key::Down,
    "left" => Key::Left,
    "right" => Key::Right,
    _ if name.len() >= 2 && name.to_ascii_lowercase().starts_with('f') => {
      let number = name[1..]
        .parse::<u8>()
        .map_err(|_| format!("invalid function key <{token}>"))?;
      if (1..=12).contains(&number) {
        Key::F(number)
      } else {
        return Err(format!("invalid function key <{token}>"));
      }
    }
    _ if name.chars().count() == 1 => {
      let character = name
        .chars()
        .next()
        .ok_or_else(|| format!("invalid key <{token}>"))?;
      if character.is_ascii_uppercase() {
        shift = true;
      }
      Key::Char(character.to_ascii_lowercase())
    }
    _ => return Err(format!("unknown key name '{name}' in <{token}>")),
  };
  Ok(KeyStroke {
    key,
    ctrl,
    alt,
    shift,
  })
}

pub fn normalize_prefix_key(input: &str) -> Result<KeyStroke, String> {
  let sequence = parse_key_notation(input)?;
  if sequence.len() != 1 {
    return Err("prefix_key must specify exactly one key".to_owned());
  }
  sequence
    .into_iter()
    .next()
    .ok_or_else(|| "prefix_key cannot be empty".to_owned())
}

pub fn key_stroke_to_string(stroke: &KeyStroke) -> String {
  if !stroke.ctrl
    && !stroke.alt
    && !stroke.shift
    && let Key::Char(character) = stroke.key
  {
    return character.to_string();
  }
  let mut parts = Vec::new();
  if stroke.ctrl {
    parts.push("C".to_owned());
  }
  if stroke.alt {
    parts.push("M".to_owned());
  }
  if stroke.shift {
    parts.push("S".to_owned());
  }
  let name = match stroke.key {
    Key::Char(character) => character.to_string(),
    Key::Enter => "CR".to_owned(),
    Key::Escape => "Esc".to_owned(),
    Key::Space => "Space".to_owned(),
    Key::Tab => "Tab".to_owned(),
    Key::Backspace => "BS".to_owned(),
    Key::Delete => "Del".to_owned(),
    Key::Insert => "Ins".to_owned(),
    Key::Home => "Home".to_owned(),
    Key::End => "End".to_owned(),
    Key::PageUp => "PageUp".to_owned(),
    Key::PageDown => "PageDown".to_owned(),
    Key::Up => "Up".to_owned(),
    Key::Down => "Down".to_owned(),
    Key::Left => "Left".to_owned(),
    Key::Right => "Right".to_owned(),
    Key::F(number) => format!("F{number}"),
  };
  parts.push(name);
  format!("<{}>", parts.join("-"))
}

pub fn key_seq_to_string(sequence: &[KeyStroke]) -> String {
  sequence.iter().map(key_stroke_to_string).collect()
}

pub fn encode_key(event: CrosstermKeyEvent) -> Option<Vec<u8>> {
  let bytes = match (event.code, event.modifiers) {
    (KeyCode::Char(character), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
      let character = character.to_ascii_lowercase();
      if character.is_ascii_lowercase() {
        vec![character as u8 - b'a' + 1]
      } else if character == ' ' {
        vec![0]
      } else {
        return None;
      }
    }
    (KeyCode::Char(character), modifiers) if modifiers.contains(KeyModifiers::ALT) => {
      let mut output = vec![0x1b];
      output.extend(character.to_string().as_bytes());
      output
    }
    (KeyCode::Char(character), modifiers) => {
      let character = if modifiers.contains(KeyModifiers::SHIFT) {
        character.to_ascii_uppercase()
      } else {
        character
      };
      character.to_string().into_bytes()
    }
    (KeyCode::Enter, _) => vec![b'\r'],
    (KeyCode::Esc, _) => vec![0x1b],
    (KeyCode::Backspace, _) => vec![0x7f],
    (KeyCode::Tab, _) => vec![b'\t'],
    (KeyCode::Left, _) => b"\x1b[D".to_vec(),
    (KeyCode::Right, _) => b"\x1b[C".to_vec(),
    (KeyCode::Up, _) => b"\x1b[A".to_vec(),
    (KeyCode::Down, _) => b"\x1b[B".to_vec(),
    (KeyCode::Home, _) => b"\x1b[H".to_vec(),
    (KeyCode::End, _) => b"\x1b[F".to_vec(),
    (KeyCode::Delete, _) => b"\x1b[3~".to_vec(),
    (KeyCode::PageUp, _) => b"\x1b[5~".to_vec(),
    (KeyCode::PageDown, _) => b"\x1b[6~".to_vec(),
    _ => return None,
  };
  Some(bytes)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_literal_and_notation_sequences() {
    assert_eq!(parse_key_notation("tw").expect("keys").len(), 2);
    assert_eq!(parse_key_notation("<C-c>aa").expect("keys").len(), 3);
    assert_eq!(parse_key_notation("<C-Space>").expect("key")[0].key, Key::Space);
    assert_eq!(parse_key_notation("<M-w>").expect("key")[0].alt, true);
    assert_eq!(parse_key_notation("<S-Up>").expect("key")[0].key, Key::Up);
    assert_eq!(parse_key_notation("<F12>").expect("key")[0].key, Key::F(12));
    assert_eq!(parse_key_notation("<C-->").expect("key")[0].key, Key::Char('-'));
    assert_eq!(parse_key_notation("<lt>").expect("key")[0].key, Key::Char('<'));
    assert_eq!(parse_key_notation("<Bar>").expect("key")[0].key, Key::Char('|'));
  }

  #[test]
  fn keeps_case_sensitive_plain_keys() {
    let sequence = parse_key_notation("tT").expect("keys");
    assert!(!sequence[0].shift);
    assert!(sequence[1].shift);
    assert_eq!(key_seq_to_string(&sequence), "t<S-t>");
  }

  #[test]
  fn rejects_invalid_or_multi_key_prefixes() {
    assert!(parse_key_notation("").is_err());
    assert!(parse_key_notation("<C-c").is_err());
    assert!(parse_key_notation("<Unknown>").is_err());
    assert!(normalize_prefix_key("<C-c>a").is_err());
    assert!(normalize_prefix_key("ctrl-space").is_err());
  }

  #[test]
  fn converts_terminal_keys() {
    let event = CrosstermKeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(
      KeyStroke::from_crossterm(event),
      Some(KeyStroke {
        key: Key::Char('c'),
        ctrl: true,
        alt: false,
        shift: false
      })
    );
    assert_eq!(encode_key(event), Some(vec![3]));
    let uppercase = CrosstermKeyEvent::new(KeyCode::Char('Z'), KeyModifiers::SHIFT);
    assert_eq!(encode_key(uppercase), Some(b"Z".to_vec()));

    let function_key = CrosstermKeyEvent::new(KeyCode::F(12), KeyModifiers::NONE);
    assert_eq!(
      KeyStroke::from_crossterm(function_key),
      Some(KeyStroke {
        key: Key::F(12),
        ctrl: false,
        alt: false,
        shift: false,
      })
    );
  }
}
