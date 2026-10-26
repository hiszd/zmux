use std::time::{
  Duration,
  Instant,
};

use crossterm::event::{
  KeyEvent,
  KeyEventKind,
};
use mlua::Function;

use super::{
  binding_trie::{
    Binding,
    BindingTrie,
    TrieNode,
  },
  key_notation::{
    self,
    Key,
    KeySeq,
    KeyStroke,
    key_seq_to_string,
  },
};

#[derive(Clone, Default)]
pub struct KeyMap {
  bindings: BindingTrie,
}

impl KeyMap {
  pub fn bind(
    &mut self,
    sequence: KeySeq,
    callback: Function,
    desc: Option<String>,
  ) -> Result<(), String> {
    if sequence.iter().any(|stroke| stroke.key == Key::Escape) {
      return Err("Escape is reserved for cancelling leader mode and cannot be bound".to_owned());
    }
    self.bindings.bind(&sequence, Binding { callback, desc })
  }

  pub fn unbind(&mut self, sequence: &[KeyStroke]) -> bool { self.bindings.unbind(sequence) }

  pub fn entries(&self) -> Vec<(String, Option<String>)> {
    let mut entries: Vec<_> = self
      .bindings
      .entries()
      .into_iter()
      .map(|(sequence, desc)| (key_seq_to_string(&sequence), desc))
      .collect();
    entries.sort_by(|(left, _), (right, _)| left.cmp(right));
    entries
  }

  #[cfg(test)]
  pub fn binding_for(&self, sequence: &[KeyStroke]) -> Option<Function> {
    self
      .bindings
      .node_for(sequence)?
      .binding
      .as_ref()
      .map(|binding| binding.callback.clone())
  }

  #[cfg(test)]
  pub fn binding_count(&self) -> usize { self.bindings.key_count() }

  fn node_for(&self, sequence: &[KeyStroke]) -> Option<&TrieNode> {
    self.bindings.node_for(sequence)
  }

  fn completions(&self, sequence: &[KeyStroke]) -> Vec<(KeyStroke, Option<String>, bool)> {
    self.bindings.completions(sequence)
  }
}

pub enum KeyOutcome {
  PrefixEntered,
  PrefixCancelled,
  Execute(Function),
  Forward(Vec<u8>),
  Unbound(String),
  Ignored,
}

pub enum KeyTimeout {
  Execute(Function),
  LeaderExpired,
}

pub struct KeyDispatcher {
  prefix_key: KeyStroke,
  sequence_timeout: Duration,
  keymap: KeyMap,
  sequence: KeySeq,
  leader_active: bool,
  deadline: Option<Instant>,
  pending_callback: Option<Function>,
}

impl KeyDispatcher {
  pub fn new(prefix_key: KeyStroke, sequence_timeout: Duration, keymap: KeyMap) -> Self {
    Self {
      prefix_key,
      sequence_timeout,
      keymap,
      sequence: Vec::new(),
      leader_active: false,
      deadline: None,
      pending_callback: None,
    }
  }

  pub fn is_leader(&self) -> bool { self.leader_active }

  pub fn is_prefix_key(&self, event: KeyEvent) -> bool {
    KeyStroke::from_crossterm(event).is_some_and(|key| key == self.prefix_key)
  }

  pub fn sequence(&self) -> &[KeyStroke] { &self.sequence }

  pub fn completions(&self) -> Vec<(KeyStroke, Option<String>, bool)> {
    if self.is_leader() {
      self.keymap.completions(&self.sequence)
    } else {
      Vec::new()
    }
  }

  pub fn time_until_timeout(&self, now: Instant) -> Option<Duration> {
    self
      .deadline
      .map(|deadline| deadline.saturating_duration_since(now))
  }

  pub fn expire(&mut self, now: Instant) -> Option<KeyTimeout> {
    if self.deadline.is_none_or(|deadline| now < deadline) {
      return None;
    }
    self.deadline = None;
    self.leader_active = false;
    self.sequence.clear();
    if let Some(callback) = self.pending_callback.take() {
      Some(KeyTimeout::Execute(callback))
    } else {
      Some(KeyTimeout::LeaderExpired)
    }
  }

  pub fn handle_event(&mut self, event: KeyEvent, now: Instant) -> KeyOutcome {
    if !matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
      return KeyOutcome::Ignored;
    }
    let Some(key) = KeyStroke::from_crossterm(event) else {
      return KeyOutcome::Ignored;
    };

    if self.deadline.is_some_and(|deadline| now >= deadline)
      && let Some(KeyTimeout::Execute(callback)) = self.expire(now)
    {
      // An expired complete mapping wins; don't reinterpret the arriving key.
      return KeyOutcome::Execute(callback);
    }

    if !self.is_leader() {
      if key == self.prefix_key {
        self.sequence.clear();
        self.pending_callback = None;
        self.leader_active = true;
        self.arm_timeout(now);
        return KeyOutcome::PrefixEntered;
      }
      return key_notation::encode_key(event).map_or(KeyOutcome::Ignored, KeyOutcome::Forward);
    }

    if key.key == Key::Escape && !key.ctrl && !key.alt {
      self.cancel();
      return KeyOutcome::PrefixCancelled;
    }

    self.sequence.push(key);
    let Some((has_children, callback)) = self.keymap.node_for(&self.sequence).map(|node| {
      (
        !node.children.is_empty(),
        node
          .binding
          .as_ref()
          .map(|binding| binding.callback.clone()),
      )
    }) else {
      if let Some(callback) = self.pending_callback.take() {
        if self.sequence_timeout.is_zero() {
          self.sequence.clear();
          self.deadline = None;
        } else {
          self.cancel();
        }
        return KeyOutcome::Execute(callback);
      }
      let typed = key_notation::key_stroke_to_string(&key);
      self.cancel();
      return KeyOutcome::Unbound(typed);
    };

    self.arm_timeout(now);
    if let Some(callback) = callback {
      if !has_children {
        self.finish_binding();
        return KeyOutcome::Execute(callback);
      }
      self.pending_callback = Some(callback);
    }
    KeyOutcome::Ignored
  }

  fn arm_timeout(&mut self, now: Instant) {
    self.deadline = (!self.sequence_timeout.is_zero()).then_some(now + self.sequence_timeout);
  }

  fn finish_binding(&mut self) { self.cancel(); }

  fn cancel(&mut self) {
    self.sequence.clear();
    self.pending_callback = None;
    self.leader_active = false;
    self.deadline = None;
  }
}

#[cfg(test)]
mod tests {
  use crossterm::event::{
    KeyCode,
    KeyEventKind,
    KeyEventState,
    KeyModifiers,
  };
  use mlua::Lua;

  use super::*;
  use crate::input::key_notation::{
    normalize_prefix_key,
    parse_key_notation,
  };

  fn dispatcher(bindings: &[(&str, &'static str)]) -> (Lua, KeyDispatcher) {
    dispatcher_with_timeout(bindings, Duration::from_millis(700))
  }

  fn dispatcher_with_timeout(
    bindings: &[(&str, &'static str)],
    sequence_timeout: Duration,
  ) -> (Lua, KeyDispatcher) {
    let lua = Lua::new();
    let mut keymap = KeyMap::default();
    for (sequence, marker) in bindings {
      let marker = *marker;
      let callback = lua
        .create_function(move |lua, ()| lua.globals().set("called", marker))
        .expect("Lua callback");
      keymap
        .bind(
          parse_key_notation(sequence).expect("valid sequence"),
          callback,
          Some(format!("{marker} action")),
        )
        .expect("bind key");
    }
    (
      lua,
      KeyDispatcher::new(
        normalize_prefix_key("<C-Space>").expect("prefix"),
        sequence_timeout,
        keymap,
      ),
    )
  }

  fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent { KeyEvent::new(code, modifiers) }

  #[test]
  fn dispatcher_routes_every_unicode_scalar_under_every_crossterm_modifier_mask() {
    let mut dispatcher = KeyDispatcher::new(
      normalize_prefix_key("<F12>").expect("prefix key"),
      Duration::from_millis(700),
      KeyMap::default(),
    );
    let now = Instant::now();
    let unsupported_modifiers = KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META;

    for codepoint in 0..=0x10ffff {
      let Some(character) = char::from_u32(codepoint) else {
        continue;
      };
      for modifier_bits in 0..=KeyModifiers::all().bits() {
        let event = key(KeyCode::Char(character), KeyModifiers::from_bits_retain(modifier_bits));
        let expected = if event.modifiers.intersects(unsupported_modifiers) {
          None
        } else {
          crate::input::key_notation::encode_key(event)
        };
        let outcome = dispatcher.handle_event(event, now);
        if let Some(expected) = expected {
          let KeyOutcome::Forward(actual) = outcome else {
            panic!("U+{codepoint:04X} with {:?} should be forwarded", event.modifiers);
          };
          assert_eq!(
            actual, expected,
            "forwarded bytes for U+{codepoint:04X} with {:?}",
            event.modifiers
          );
        } else if !matches!(outcome, KeyOutcome::Ignored) {
          panic!("U+{codepoint:04X} with {:?} should be ignored", event.modifiers);
        }
      }
    }
  }

  #[test]
  fn dispatcher_routes_every_non_character_key_mask_kind_and_state_correctly() {
    let prefix = KeyStroke {
      key: crate::input::key_notation::Key::Char(char::MAX),
      ctrl: false,
      alt: false,
      shift: false,
    };
    let mut dispatcher = KeyDispatcher::new(prefix, Duration::ZERO, KeyMap::default());
    let now = Instant::now();
    let unsupported_modifiers = KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META;

    for code in crate::input::key_notation::all_non_character_key_codes() {
      for modifier_bits in 0..=KeyModifiers::all().bits() {
        let modifiers = KeyModifiers::from_bits_retain(modifier_bits);
        for kind in [KeyEventKind::Press, KeyEventKind::Repeat, KeyEventKind::Release] {
          for state_bits in 0..=KeyEventState::all().bits() {
            let event = KeyEvent::new_with_kind_and_state(
              code,
              modifiers,
              kind,
              KeyEventState::from_bits_retain(state_bits),
            );
            let expected =
              if kind == KeyEventKind::Release || modifiers.intersects(unsupported_modifiers) {
                None
              } else {
                crate::input::key_notation::encode_key(event)
              };
            let outcome = dispatcher.handle_event(event, now);
            match (outcome, expected) {
              (KeyOutcome::Forward(actual), Some(expected)) => assert_eq!(
                actual, expected,
                "PTY bytes for {code:?}, {modifiers:?}, {kind:?}, state={state_bits:#05b}"
              ),
              (KeyOutcome::Ignored, None) => {}
              (KeyOutcome::Ignored, Some(_)) => {
                panic!("{code:?}, {modifiers:?}, {kind:?}, state={state_bits:#05b} should forward")
              }
              (KeyOutcome::Forward(_), None) => panic!(
                "{code:?}, {modifiers:?}, {kind:?}, state={state_bits:#05b} should be ignored"
              ),
              _ => panic!(
                "{code:?}, {modifiers:?}, {kind:?}, state={state_bits:#05b} entered leader mode"
              ),
            }
          }
        }
      }
    }
  }

  #[test]
  fn character_event_kinds_and_keyboard_states_do_not_change_routing() {
    let mut dispatcher = KeyDispatcher::new(
      normalize_prefix_key("<F12>").expect("prefix key"),
      Duration::from_millis(700),
      KeyMap::default(),
    );
    let now = Instant::now();
    let unsupported_modifiers = KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META;
    for character in ['_', ' ', '\0', 'é', '界', '🦀', char::MAX] {
      for modifier_bits in 0..=KeyModifiers::all().bits() {
        let modifiers = KeyModifiers::from_bits_retain(modifier_bits);
        for kind in [KeyEventKind::Press, KeyEventKind::Repeat, KeyEventKind::Release] {
          for state_bits in 0..=KeyEventState::all().bits() {
            let event = KeyEvent::new_with_kind_and_state(
              KeyCode::Char(character),
              modifiers,
              kind,
              KeyEventState::from_bits_retain(state_bits),
            );
            let expected =
              if kind == KeyEventKind::Release || modifiers.intersects(unsupported_modifiers) {
                None
              } else {
                crate::input::key_notation::encode_key(event)
              };
            let outcome = dispatcher.handle_event(event, now);
            match (outcome, expected) {
              (KeyOutcome::Forward(actual), Some(expected)) => assert_eq!(actual, expected),
              (KeyOutcome::Ignored, None) => {}
              (KeyOutcome::Ignored, Some(_)) => {
                panic!("{event:?} should be forwarded")
              }
              (KeyOutcome::Forward(_), None) => panic!("{event:?} should be ignored"),
              _ => panic!("{event:?} entered leader mode"),
            }
          }
        }
      }
    }
  }

  fn press_prefix(dispatcher: &mut KeyDispatcher, now: Instant) {
    assert!(matches!(
      dispatcher.handle_event(key(KeyCode::Char(' '), KeyModifiers::CONTROL), now),
      KeyOutcome::PrefixEntered
    ));
  }

  // Developer approved: do not remove.
  #[test]
  fn unbound_keys_are_not_forwarded_after_prefix() {
    let (_lua, mut dispatcher) = dispatcher(&[]);
    let now = Instant::now();
    press_prefix(&mut dispatcher, now);
    assert!(matches!(
        dispatcher.handle_event(key(KeyCode::Char('z'), KeyModifiers::NONE), now),
        KeyOutcome::Unbound(value) if value == "z"
    ));
    assert!(!dispatcher.is_leader());
  }

  // Developer approved: do not remove.
  #[test]
  fn unbound_key_leaves_infinite_leader_mode() {
    let (_lua, mut dispatcher) = dispatcher_with_timeout(&[], Duration::ZERO);
    let now = Instant::now();
    press_prefix(&mut dispatcher, now);
    assert!(dispatcher.is_leader());
    let after_long_wait = now + Duration::from_secs(60);
    assert!(dispatcher.expire(after_long_wait).is_none());
    assert!(dispatcher.is_leader());
    assert!(matches!(
        dispatcher.handle_event(key(KeyCode::Char('z'), KeyModifiers::NONE), after_long_wait),
        KeyOutcome::Unbound(value) if value == "z"
    ));
    assert!(!dispatcher.is_leader());
  }

  // Developer approved: do not remove.
  #[test]
  fn prefix_is_consumed_and_regular_terminal_keys_are_forwarded() {
    let (_lua, mut dispatcher) = dispatcher(&[]);
    let now = Instant::now();
    assert!(matches!(
        dispatcher.handle_event(key(KeyCode::Char('z'), KeyModifiers::NONE), now),
        KeyOutcome::Forward(bytes) if bytes == b"z"
    ));
    press_prefix(&mut dispatcher, now);
    assert!(dispatcher.is_leader());
  }

  // Developer approved: do not remove.
  #[test]
  fn terminal_input_forwarding_handles_special_keys_modifiers_and_event_kinds() {
    let mut dispatcher = KeyDispatcher::new(
      normalize_prefix_key("<C-x>").expect("prefix key"),
      Duration::from_millis(700),
      KeyMap::default(),
    );
    let now = Instant::now();
    let cases = [
      (KeyCode::Char('`'), KeyModifiers::SHIFT, &b"~"[..]),
      (KeyCode::Char('1'), KeyModifiers::SHIFT, &b"!"[..]),
      (KeyCode::Char('2'), KeyModifiers::SHIFT, &b"@"[..]),
      (KeyCode::Char('3'), KeyModifiers::SHIFT, &b"#"[..]),
      (KeyCode::Char('4'), KeyModifiers::SHIFT, &b"$"[..]),
      (KeyCode::Char('5'), KeyModifiers::SHIFT, &b"%"[..]),
      (KeyCode::Char('6'), KeyModifiers::SHIFT, &b"^"[..]),
      (KeyCode::Char('7'), KeyModifiers::SHIFT, &b"&"[..]),
      (KeyCode::Char('8'), KeyModifiers::SHIFT, &b"*"[..]),
      (KeyCode::Char('9'), KeyModifiers::SHIFT, &b"("[..]),
      (KeyCode::Char('0'), KeyModifiers::SHIFT, &b")"[..]),
      (KeyCode::Char('-'), KeyModifiers::SHIFT, &b"_"[..]),
      (KeyCode::Char('='), KeyModifiers::SHIFT, &b"+"[..]),
      (KeyCode::Char('['), KeyModifiers::SHIFT, &b"{"[..]),
      (KeyCode::Char(']'), KeyModifiers::SHIFT, &b"}"[..]),
      (KeyCode::Char('\\'), KeyModifiers::SHIFT, &b"|"[..]),
      (KeyCode::Char(';'), KeyModifiers::SHIFT, &b":"[..]),
      (KeyCode::Char('\''), KeyModifiers::SHIFT, &[b'"'][..]),
      (KeyCode::Char(','), KeyModifiers::SHIFT, &b"<"[..]),
      (KeyCode::Char('.'), KeyModifiers::SHIFT, &b">"[..]),
      (KeyCode::Char('/'), KeyModifiers::SHIFT, &b"?"[..]),
      (KeyCode::Char('v'), KeyModifiers::SHIFT, &b"V"[..]),
      (KeyCode::Char('A'), KeyModifiers::NONE, &b"A"[..]),
      (KeyCode::Char('`'), KeyModifiers::ALT | KeyModifiers::SHIFT, &b"\x1b~"[..]),
      (KeyCode::Char('x'), KeyModifiers::ALT, &b"\x1bx"[..]),
      (KeyCode::Char(' '), KeyModifiers::CONTROL, &b"\0"[..]),
      (KeyCode::Char('c'), KeyModifiers::CONTROL, &b"\x03"[..]),
      (KeyCode::Char('@'), KeyModifiers::CONTROL, &b"\0"[..]),
      (KeyCode::Char('['), KeyModifiers::CONTROL, &b"\x1b"[..]),
      (KeyCode::Char('\\'), KeyModifiers::CONTROL, &b"\x1c"[..]),
      (KeyCode::Char(']'), KeyModifiers::CONTROL, &b"\x1d"[..]),
      (KeyCode::Char('^'), KeyModifiers::CONTROL, &b"\x1e"[..]),
      (KeyCode::Char('_'), KeyModifiers::CONTROL, &b"\x1f"[..]),
      (KeyCode::Char('?'), KeyModifiers::CONTROL, &b"\x7f"[..]),
      (KeyCode::Char('2'), KeyModifiers::CONTROL | KeyModifiers::SHIFT, &b"\0"[..]),
      (KeyCode::Char('c'), KeyModifiers::CONTROL | KeyModifiers::ALT, &b"\x1b\x03"[..]),
      (KeyCode::Char('界'), KeyModifiers::NONE, "界".as_bytes()),
      (KeyCode::Null, KeyModifiers::NONE, &b"\0"[..]),
      (KeyCode::Null, KeyModifiers::ALT, &b"\x1b\0"[..]),
      (KeyCode::Enter, KeyModifiers::NONE, &b"\r"[..]),
      (KeyCode::Enter, KeyModifiers::ALT, &b"\x1b\r"[..]),
      (KeyCode::Esc, KeyModifiers::NONE, &b"\x1b"[..]),
      (KeyCode::Esc, KeyModifiers::ALT, &b"\x1b\x1b"[..]),
      (KeyCode::Backspace, KeyModifiers::NONE, &b"\x7f"[..]),
      (KeyCode::Backspace, KeyModifiers::CONTROL, &b"\x08"[..]),
      (KeyCode::Backspace, KeyModifiers::ALT, &b"\x1b\x7f"[..]),
      (KeyCode::Tab, KeyModifiers::NONE, &b"\t"[..]),
      (KeyCode::Tab, KeyModifiers::SHIFT, &b"\x1b[Z"[..]),
      (KeyCode::Tab, KeyModifiers::SHIFT | KeyModifiers::CONTROL, &b"\x1b[1;6Z"[..]),
      (KeyCode::Tab, KeyModifiers::SHIFT | KeyModifiers::ALT, &b"\x1b[1;4Z"[..]),
      (
        KeyCode::Tab,
        KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL,
        &b"\x1b[1;8Z"[..],
      ),
      (KeyCode::Tab, KeyModifiers::ALT | KeyModifiers::CONTROL, &b"\x1b[1;7I"[..]),
      (KeyCode::BackTab, KeyModifiers::NONE, &b"\x1b[Z"[..]),
      (KeyCode::Insert, KeyModifiers::NONE, &b"\x1b[2~"[..]),
      (KeyCode::F(1), KeyModifiers::NONE, &b"\x1bOP"[..]),
      (KeyCode::F(2), KeyModifiers::NONE, &b"\x1bOQ"[..]),
      (KeyCode::F(3), KeyModifiers::NONE, &b"\x1bOR"[..]),
      (KeyCode::F(4), KeyModifiers::NONE, &b"\x1bOS"[..]),
      (KeyCode::F(5), KeyModifiers::NONE, &b"\x1b[15~"[..]),
      (KeyCode::F(6), KeyModifiers::NONE, &b"\x1b[17~"[..]),
      (KeyCode::F(7), KeyModifiers::NONE, &b"\x1b[18~"[..]),
      (KeyCode::F(8), KeyModifiers::NONE, &b"\x1b[19~"[..]),
      (KeyCode::F(9), KeyModifiers::NONE, &b"\x1b[20~"[..]),
      (KeyCode::F(10), KeyModifiers::NONE, &b"\x1b[21~"[..]),
      (KeyCode::F(11), KeyModifiers::NONE, &b"\x1b[23~"[..]),
      (KeyCode::F(12), KeyModifiers::NONE, &b"\x1b[24~"[..]),
      (KeyCode::Left, KeyModifiers::NONE, &b"\x1b[D"[..]),
      (KeyCode::Left, KeyModifiers::CONTROL, &b"\x1b[1;5D"[..]),
      (KeyCode::Right, KeyModifiers::NONE, &b"\x1b[C"[..]),
      (
        KeyCode::Right,
        KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL,
        &b"\x1b[1;8C"[..],
      ),
      (KeyCode::Up, KeyModifiers::NONE, &b"\x1b[A"[..]),
      (KeyCode::Up, KeyModifiers::SHIFT, &b"\x1b[1;2A"[..]),
      (KeyCode::Down, KeyModifiers::NONE, &b"\x1b[B"[..]),
      (KeyCode::Home, KeyModifiers::NONE, &b"\x1b[H"[..]),
      (KeyCode::Home, KeyModifiers::SHIFT, &b"\x1b[1;2H"[..]),
      (KeyCode::End, KeyModifiers::NONE, &b"\x1b[F"[..]),
      (KeyCode::Delete, KeyModifiers::NONE, &b"\x1b[3~"[..]),
      (KeyCode::Delete, KeyModifiers::CONTROL, &b"\x1b[3;5~"[..]),
      (KeyCode::Insert, KeyModifiers::ALT, &b"\x1b[2;3~"[..]),
      (KeyCode::PageUp, KeyModifiers::NONE, &b"\x1b[5~"[..]),
      (KeyCode::PageUp, KeyModifiers::ALT, &b"\x1b[5;3~"[..]),
      (KeyCode::PageDown, KeyModifiers::NONE, &b"\x1b[6~"[..]),
      (KeyCode::Tab, KeyModifiers::CONTROL, &b"\x1b[1;5I"[..]),
      (KeyCode::BackTab, KeyModifiers::CONTROL, &b"\x1b[1;6Z"[..]),
      (KeyCode::BackTab, KeyModifiers::ALT | KeyModifiers::CONTROL, &b"\x1b[1;8Z"[..]),
      (KeyCode::F(1), KeyModifiers::SHIFT, &b"\x1b[1;2P"[..]),
      (KeyCode::F(5), KeyModifiers::CONTROL, &b"\x1b[15;5~"[..]),
    ];
    for (code, modifiers, expected) in cases {
      let KeyOutcome::Forward(bytes) = dispatcher.handle_event(key(code, modifiers), now) else {
        panic!("{code:?} with {modifiers:?} should be forwarded to the PTY");
      };
      assert_eq!(bytes.as_slice(), expected, "{code:?} with {modifiers:?}");
    }
    for letter in b'a'..=b'z' {
      let code = KeyCode::Char(char::from(letter));
      let KeyOutcome::Forward(bytes) = dispatcher.handle_event(key(code, KeyModifiers::SHIFT), now)
      else {
        panic!("Shift+{code:?} should be forwarded to the PTY");
      };
      assert_eq!(bytes, [letter.to_ascii_uppercase()], "Shift+{code:?}");
    }

    let mut release = key(KeyCode::Char('r'), KeyModifiers::NONE);
    release.kind = crossterm::event::KeyEventKind::Release;
    assert!(matches!(dispatcher.handle_event(release, now), KeyOutcome::Ignored));

    let mut repeat = key(KeyCode::Char('r'), KeyModifiers::NONE);
    repeat.kind = crossterm::event::KeyEventKind::Repeat;
    let KeyOutcome::Forward(bytes) = dispatcher.handle_event(repeat, now) else {
      panic!("repeated keys should be forwarded to the PTY");
    };
    assert_eq!(bytes, b"r");
  }

  #[test]
  fn unsupported_modifiers_are_not_misrouted_as_plain_keys() {
    let mut dispatcher = KeyDispatcher::new(
      normalize_prefix_key("<C-Space>").expect("prefix key"),
      Duration::from_millis(700),
      KeyMap::default(),
    );
    let now = Instant::now();

    for modifiers in [KeyModifiers::SUPER, KeyModifiers::HYPER, KeyModifiers::META] {
      assert!(matches!(
        dispatcher.handle_event(key(KeyCode::Char('x'), modifiers), now),
        KeyOutcome::Ignored
      ));
    }
  }

  #[test]
  fn cursor_keys_preserve_all_ctrl_alt_and_shift_combinations() {
    let mut dispatcher = KeyDispatcher::new(
      normalize_prefix_key("<C-Space>").expect("prefix key"),
      Duration::from_millis(700),
      KeyMap::default(),
    );
    let now = Instant::now();
    let modifiers = [
      (KeyModifiers::SHIFT, 2),
      (KeyModifiers::ALT, 3),
      (KeyModifiers::SHIFT | KeyModifiers::ALT, 4),
      (KeyModifiers::CONTROL, 5),
      (KeyModifiers::SHIFT | KeyModifiers::CONTROL, 6),
      (KeyModifiers::ALT | KeyModifiers::CONTROL, 7),
      (KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL, 8),
    ];
    let cursor_keys = [
      (KeyCode::Left, 'D'),
      (KeyCode::Right, 'C'),
      (KeyCode::Up, 'A'),
      (KeyCode::Down, 'B'),
      (KeyCode::Home, 'H'),
      (KeyCode::End, 'F'),
    ];

    for (code, final_character) in cursor_keys {
      for (modifiers, parameter) in modifiers {
        let expected = format!("\x1b[1;{parameter}{final_character}").into_bytes();
        let KeyOutcome::Forward(bytes) = dispatcher.handle_event(key(code, modifiers), now) else {
          panic!("{code:?} with {modifiers:?} should be forwarded to the PTY");
        };
        assert_eq!(bytes, expected, "{code:?} with {modifiers:?}");
      }
    }
  }

  #[test]
  fn function_and_tilde_keys_preserve_all_ctrl_alt_and_shift_combinations() {
    let mut dispatcher = KeyDispatcher::new(
      normalize_prefix_key("<C-Space>").expect("prefix key"),
      Duration::from_millis(700),
      KeyMap::default(),
    );
    let now = Instant::now();
    let modifiers = [
      (KeyModifiers::SHIFT, 2),
      (KeyModifiers::ALT, 3),
      (KeyModifiers::SHIFT | KeyModifiers::ALT, 4),
      (KeyModifiers::CONTROL, 5),
      (KeyModifiers::SHIFT | KeyModifiers::CONTROL, 6),
      (KeyModifiers::ALT | KeyModifiers::CONTROL, 7),
      (KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL, 8),
    ];
    let function_keys = [
      (KeyCode::F(1), Some('P'), None),
      (KeyCode::F(2), Some('Q'), None),
      (KeyCode::F(3), Some('R'), None),
      (KeyCode::F(4), Some('S'), None),
      (KeyCode::F(5), None, Some(15)),
      (KeyCode::F(6), None, Some(17)),
      (KeyCode::F(7), None, Some(18)),
      (KeyCode::F(8), None, Some(19)),
      (KeyCode::F(9), None, Some(20)),
      (KeyCode::F(10), None, Some(21)),
      (KeyCode::F(11), None, Some(23)),
      (KeyCode::F(12), None, Some(24)),
    ];
    for (code, final_character, tilde_code) in function_keys {
      for (modifiers, parameter) in modifiers {
        let expected = if let Some(final_character) = final_character {
          format!("\x1b[1;{parameter}{final_character}")
        } else {
          format!("\x1b[{};{parameter}~", tilde_code.expect("tilde function key"))
        }
        .into_bytes();
        let KeyOutcome::Forward(bytes) = dispatcher.handle_event(key(code, modifiers), now) else {
          panic!("{code:?} with {modifiers:?} should be forwarded to the PTY");
        };
        assert_eq!(bytes, expected, "{code:?} with {modifiers:?}");
      }
    }

    for (code, tilde_code) in
      [(KeyCode::Insert, 2), (KeyCode::Delete, 3), (KeyCode::PageUp, 5), (KeyCode::PageDown, 6)]
    {
      for (modifiers, parameter) in modifiers {
        let expected = format!("\x1b[{tilde_code};{parameter}~").into_bytes();
        let KeyOutcome::Forward(bytes) = dispatcher.handle_event(key(code, modifiers), now) else {
          panic!("{code:?} with {modifiers:?} should be forwarded to the PTY");
        };
        assert_eq!(bytes, expected, "{code:?} with {modifiers:?}");
      }
    }
  }

  #[test]
  fn shifted_and_null_key_events_match_their_key_notation_bindings() {
    let (lua, mut dispatcher) = dispatcher(&[
      ("<S-Tab>", "shift-tab"),
      ("<C-Space>", "control-space"),
      ("<S-`>", "shift-grave"),
    ]);
    let now = Instant::now();

    for (code, modifiers, expected) in [
      (KeyCode::BackTab, KeyModifiers::NONE, "shift-tab"),
      (KeyCode::Tab, KeyModifiers::SHIFT, "shift-tab"),
      (KeyCode::Null, KeyModifiers::NONE, "control-space"),
      (KeyCode::Char('`'), KeyModifiers::SHIFT, "shift-grave"),
    ] {
      press_prefix(&mut dispatcher, now);
      let KeyOutcome::Execute(callback) = dispatcher.handle_event(key(code, modifiers), now) else {
        panic!("{code:?} with {modifiers:?} should match its key binding");
      };
      callback.call::<()>(()).expect("invoke key binding");
      assert_eq!(lua.globals().get::<String>("called").expect("callback ran"), expected);
    }
  }

  #[test]
  fn control_letter_keys_map_to_their_terminal_control_bytes() {
    let mut dispatcher = KeyDispatcher::new(
      normalize_prefix_key("<C-Space>").expect("prefix key"),
      Duration::from_millis(700),
      KeyMap::default(),
    );
    let now = Instant::now();

    for (letter, expected) in (b'a'..=b'z').zip(1u8..=26) {
      let code = KeyCode::Char(char::from(letter));
      let KeyOutcome::Forward(bytes) =
        dispatcher.handle_event(key(code, KeyModifiers::CONTROL), now)
      else {
        panic!("Ctrl+{code:?} should be forwarded to the PTY");
      };
      assert_eq!(bytes, [expected], "Ctrl+{code:?}");

      let uppercase = KeyCode::Char(char::from(letter.to_ascii_uppercase()));
      let KeyOutcome::Forward(bytes) =
        dispatcher.handle_event(key(uppercase, KeyModifiers::CONTROL), now)
      else {
        panic!("Ctrl+{uppercase:?} should be forwarded to the PTY");
      };
      assert_eq!(bytes, [expected], "Ctrl+{uppercase:?}");
    }
  }

  #[test]
  fn alt_printable_ascii_characters_are_prefixed_with_escape() {
    let mut dispatcher = KeyDispatcher::new(
      normalize_prefix_key("<C-x>").expect("prefix key"),
      Duration::from_millis(700),
      KeyMap::default(),
    );
    let now = Instant::now();

    for byte in 0x20..=0x7e {
      let code = KeyCode::Char(char::from(byte));
      let KeyOutcome::Forward(bytes) = dispatcher.handle_event(key(code, KeyModifiers::ALT), now)
      else {
        panic!("Alt+{code:?} should be forwarded to the PTY");
      };
      assert_eq!(bytes, [b'\x1b', byte], "Alt+{code:?}");
    }
  }

  #[test]
  fn every_printable_ascii_character_is_forwarded_unchanged() {
    let mut dispatcher = KeyDispatcher::new(
      normalize_prefix_key("<C-x>").expect("prefix key"),
      Duration::from_millis(700),
      KeyMap::default(),
    );
    let now = Instant::now();

    for byte in 0x20..=0x7e {
      let code = KeyCode::Char(char::from(byte));
      let KeyOutcome::Forward(bytes) = dispatcher.handle_event(key(code, KeyModifiers::NONE), now)
      else {
        panic!("printable ASCII {code:?} should be forwarded to the PTY");
      };
      assert_eq!(bytes, [byte], "printable ASCII {code:?}");
    }
  }

  // Developer approved: do not remove.
  #[test]
  fn a_shorter_binding_waits_for_longer_sequence_then_timeout_executes_it() {
    let (lua, mut dispatcher) = dispatcher(&[("a", "short"), ("aa", "long")]);
    let now = Instant::now();
    assert!(dispatcher.completions().is_empty(), "popup is hidden before prefix");
    press_prefix(&mut dispatcher, now);
    assert_eq!(dispatcher.completions().len(), 1, "popup opens after prefix");
    assert!(dispatcher.completions()[0].2, "a prefix should be marked as nested");
    assert!(matches!(
      dispatcher.handle_event(key(KeyCode::Char('a'), KeyModifiers::NONE), now),
      KeyOutcome::Ignored
    ));
    assert_eq!(dispatcher.completions().len(), 1);
    let Some(KeyTimeout::Execute(callback)) = dispatcher.expire(now + Duration::from_millis(700))
    else {
      panic!("shorter mapping should execute on timeout");
    };
    assert!(dispatcher.completions().is_empty(), "popup closes after timeout");
    callback.call::<()>(()).expect("invoke shorter mapping");
    assert_eq!(lua.globals().get::<String>("called").expect("callback ran"), "short");
  }

  // Developer approved: do not remove.
  #[test]
  fn longer_sequence_wins_when_completed_within_timeout() {
    let (lua, mut dispatcher) = dispatcher(&[("a", "short"), ("aa", "long")]);
    let now = Instant::now();
    press_prefix(&mut dispatcher, now);
    dispatcher.handle_event(key(KeyCode::Char('a'), KeyModifiers::NONE), now);
    let outcome = dispatcher
      .handle_event(key(KeyCode::Char('a'), KeyModifiers::NONE), now + Duration::from_millis(699));
    let KeyOutcome::Execute(callback) = outcome else {
      panic!("longer mapping should execute immediately");
    };
    callback.call::<()>(()).expect("invoke longer mapping");
    assert_eq!(lua.globals().get::<String>("called").expect("callback ran"), "long");
  }

  #[test]
  fn default_previous_tab_binding_executes_without_waiting_for_a_longer_binding() {
    let config =
      crate::lua::load_config(crate::lua::DEFAULT_CONFIG_SOURCE).expect("default config");
    let mut dispatcher =
      KeyDispatcher::new(config.prefix_key, config.sequence_timeout, config.keymap);
    let now = Instant::now();
    press_prefix(&mut dispatcher, now);

    assert!(matches!(
      dispatcher.handle_event(key(KeyCode::Char('p'), KeyModifiers::NONE), now),
      KeyOutcome::Execute(_)
    ));
  }

  // Developer approved: do not remove.
  #[test]
  fn mismatching_key_resolves_pending_shorter_binding() {
    let (lua, mut dispatcher) = dispatcher(&[("a", "short"), ("aa", "long")]);
    let now = Instant::now();
    press_prefix(&mut dispatcher, now);
    dispatcher.handle_event(key(KeyCode::Char('a'), KeyModifiers::NONE), now);
    let outcome = dispatcher
      .handle_event(key(KeyCode::Char('b'), KeyModifiers::NONE), now + Duration::from_millis(10));
    let KeyOutcome::Execute(callback) = outcome else {
      panic!("pending shorter binding should win on mismatch");
    };
    callback.call::<()>(()).expect("invoke shorter mapping");
    assert_eq!(lua.globals().get::<String>("called").expect("callback ran"), "short");
  }
}
