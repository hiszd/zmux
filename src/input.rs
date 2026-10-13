use crate::{
    binding_trie::{Binding, BindingTrie, TrieNode},
    key_notation::{self, Key, KeySeq, KeyStroke},
};
use crossterm::event::{KeyEvent, KeyEventKind};
use mlua::Function;
use std::time::{Duration, Instant};

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
            return Err(
                "Escape is reserved for cancelling leader mode and cannot be bound".to_owned(),
            );
        }
        self.bindings.bind(&sequence, Binding { callback, desc })
    }

    pub fn unbind(&mut self, sequence: &[KeyStroke]) -> bool {
        self.bindings.unbind(sequence)
    }

    #[cfg(test)]
    pub fn binding_for(&self, sequence: &[KeyStroke]) -> Option<Function> {
        self.bindings
            .node_for(sequence)?
            .binding
            .as_ref()
            .map(|binding| binding.callback.clone())
    }

    #[cfg(test)]
    pub fn binding_count(&self) -> usize {
        self.bindings.key_count()
    }

    fn node_for(&self, sequence: &[KeyStroke]) -> Option<&TrieNode> {
        self.bindings.node_for(sequence)
    }

    fn completions(&self, sequence: &[KeyStroke]) -> Vec<(KeyStroke, Option<String>)> {
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

    pub fn is_leader(&self) -> bool {
        self.leader_active
    }

    pub fn is_prefix_key(&self, event: KeyEvent) -> bool {
        KeyStroke::from_crossterm(event).is_some_and(|key| key == self.prefix_key)
    }

    pub fn sequence(&self) -> &[KeyStroke] {
        &self.sequence
    }

    pub fn completions(&self) -> Vec<(KeyStroke, Option<String>)> {
        if self.is_leader() {
            self.keymap.completions(&self.sequence)
        } else {
            Vec::new()
        }
    }

    pub fn time_until_timeout(&self, now: Instant) -> Option<Duration> {
        self.deadline
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
            return key_notation::encode_key(event)
                .map_or(KeyOutcome::Ignored, KeyOutcome::Forward);
        }

        if key.key == Key::Escape && !key.ctrl && !key.alt {
            self.cancel();
            return KeyOutcome::PrefixCancelled;
        }

        self.sequence.push(key);
        let Some((has_children, callback)) = self.keymap.node_for(&self.sequence).map(|node| {
            (
                !node.children.is_empty(),
                node.binding
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

    fn finish_binding(&mut self) {
        self.cancel();
    }

    fn cancel(&mut self) {
        self.sequence.clear();
        self.pending_callback = None;
        self.leader_active = false;
        self.deadline = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key_notation::{normalize_prefix_key, parse_key_notation};
    use crossterm::event::{KeyCode, KeyModifiers};
    use mlua::Lua;

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

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn press_prefix(dispatcher: &mut KeyDispatcher, now: Instant) {
        assert!(matches!(
            dispatcher.handle_event(key(KeyCode::Char(' '), KeyModifiers::CONTROL), now),
            KeyOutcome::PrefixEntered
        ));
    }

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

    #[test]
    fn unbound_key_leaves_infinite_leader_mode() {
        let (_lua, mut dispatcher) = dispatcher_with_timeout(&[], Duration::ZERO);
        let now = Instant::now();
        press_prefix(&mut dispatcher, now);
        assert!(dispatcher.is_leader());
        assert!(matches!(
            dispatcher.handle_event(key(KeyCode::Char('z'), KeyModifiers::NONE), now),
            KeyOutcome::Unbound(value) if value == "z"
        ));
        assert!(!dispatcher.is_leader());
    }

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

    #[test]
    fn a_shorter_binding_waits_for_longer_sequence_then_timeout_executes_it() {
        let (lua, mut dispatcher) = dispatcher(&[("a", "short"), ("aa", "long")]);
        let now = Instant::now();
        assert!(
            dispatcher.completions().is_empty(),
            "popup is hidden before prefix"
        );
        press_prefix(&mut dispatcher, now);
        assert_eq!(
            dispatcher.completions().len(),
            1,
            "popup opens after prefix"
        );
        assert!(matches!(
            dispatcher.handle_event(key(KeyCode::Char('a'), KeyModifiers::NONE), now),
            KeyOutcome::Ignored
        ));
        assert_eq!(dispatcher.completions().len(), 1);
        let Some(KeyTimeout::Execute(callback)) =
            dispatcher.expire(now + Duration::from_millis(700))
        else {
            panic!("shorter mapping should execute on timeout");
        };
        assert!(
            dispatcher.completions().is_empty(),
            "popup closes after timeout"
        );
        callback.call::<()>(()).expect("invoke shorter mapping");
        assert_eq!(
            lua.globals().get::<String>("called").expect("callback ran"),
            "short"
        );
    }

    #[test]
    fn zero_timeout_exits_leader_mode_after_binding_completes() {
        let (lua, mut dispatcher) = dispatcher_with_timeout(&[("tw", "tab")], Duration::ZERO);
        let now = Instant::now();
        press_prefix(&mut dispatcher, now);
        assert!(dispatcher.is_leader());
        assert_eq!(dispatcher.time_until_timeout(now), None);
        assert!(dispatcher.expire(now + Duration::from_secs(60)).is_none());
        assert!(dispatcher.is_leader());

        dispatcher.handle_event(key(KeyCode::Char('t'), KeyModifiers::NONE), now);
        assert!(dispatcher.is_leader());
        assert!(dispatcher.expire(now + Duration::from_secs(120)).is_none());
        assert!(dispatcher.is_leader());

        let outcome = dispatcher.handle_event(
            key(KeyCode::Char('w'), KeyModifiers::NONE),
            now + Duration::from_secs(121),
        );
        let KeyOutcome::Execute(callback) = outcome else {
            panic!("bound sequences should still execute with timeout disabled");
        };
        callback.call::<()>(()).expect("invoke mapped callback");
        assert_eq!(
            lua.globals().get::<String>("called").expect("callback ran"),
            "tab"
        );
        assert!(!dispatcher.is_leader());
    }

    #[test]
    fn longer_sequence_wins_when_completed_within_timeout() {
        let (lua, mut dispatcher) = dispatcher(&[("a", "short"), ("aa", "long")]);
        let now = Instant::now();
        press_prefix(&mut dispatcher, now);
        dispatcher.handle_event(key(KeyCode::Char('a'), KeyModifiers::NONE), now);
        let outcome = dispatcher.handle_event(
            key(KeyCode::Char('a'), KeyModifiers::NONE),
            now + Duration::from_millis(699),
        );
        let KeyOutcome::Execute(callback) = outcome else {
            panic!("longer mapping should execute immediately");
        };
        callback.call::<()>(()).expect("invoke longer mapping");
        assert_eq!(
            lua.globals().get::<String>("called").expect("callback ran"),
            "long"
        );
    }

    #[test]
    fn mismatching_key_resolves_pending_shorter_binding() {
        let (lua, mut dispatcher) = dispatcher(&[("a", "short"), ("aa", "long")]);
        let now = Instant::now();
        press_prefix(&mut dispatcher, now);
        dispatcher.handle_event(key(KeyCode::Char('a'), KeyModifiers::NONE), now);
        let outcome = dispatcher.handle_event(
            key(KeyCode::Char('b'), KeyModifiers::NONE),
            now + Duration::from_millis(10),
        );
        let KeyOutcome::Execute(callback) = outcome else {
            panic!("pending shorter binding should win on mismatch");
        };
        callback.call::<()>(()).expect("invoke shorter mapping");
        assert_eq!(
            lua.globals().get::<String>("called").expect("callback ran"),
            "short"
        );
    }
}
