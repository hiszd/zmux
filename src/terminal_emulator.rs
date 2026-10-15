use std::{
  cell::RefCell,
  io::Write,
  rc::Rc,
  sync::Arc,
};

use alacritty_terminal::{
  event::{
    Event,
    EventListener,
    WindowSize,
  },
  grid::{
    Dimensions,
    Grid,
  },
  term::{
    Config,
    Osc52,
    Term,
    cell::Cell,
  },
  vte::ansi::Processor,
};

struct TerminalSize {
  rows: usize,
  columns: usize,
}

impl Dimensions for TerminalSize {
  fn total_lines(&self) -> usize { self.rows + 10_000 }
  fn screen_lines(&self) -> usize { self.rows }
  fn columns(&self) -> usize { self.columns }
}

#[derive(Clone, Default)]
pub struct EventProxy {
  events: Rc<RefCell<Vec<Event>>>,
}

impl EventListener for EventProxy {
  fn send_event(&self, event: Event) { self.events.borrow_mut().push(event); }
}

type ClipboardFormatter = Arc<dyn Fn(&str) -> String + Sync + Send + 'static>;

#[derive(Default)]
pub struct EmulatorEvents {
  pub title: Option<String>,
  pub clipboard_stores: Vec<String>,
  pub clipboard_loads: Vec<ClipboardFormatter>,
}

pub struct TerminalEmulator {
  term: Term<EventProxy>,
  parser: Processor,
  events: EventProxy,
  title: Option<String>,
}

impl TerminalEmulator {
  pub fn new(rows: u16, columns: u16) -> Self {
    let events = EventProxy::default();
    let config = Config {
      scrolling_history: 10_000,
      osc52: Osc52::CopyPaste,
      ..Config::default()
    };
    let size = TerminalSize {
      rows: rows as usize,
      columns: columns as usize,
    };
    Self {
      term: Term::new(config, &size, events.clone()),
      parser: Processor::new(),
      events,
      title: None,
    }
  }

  pub fn process(
    &mut self,
    bytes: &[u8],
    writer: &mut dyn Write,
  ) -> std::io::Result<EmulatorEvents> {
    self.parser.advance(&mut self.term, bytes);
    let mut events = EmulatorEvents::default();
    for event in self.events.events.borrow_mut().drain(..) {
      match event {
        Event::PtyWrite(text) => writer.write_all(text.as_bytes())?,
        Event::Title(title) => {
          self.title = Some(title.clone());
          events.title = Some(title);
        }
        Event::ResetTitle => {
          self.title = None;
          events.title = Some(String::new());
        }
        Event::TextAreaSizeRequest(format) => {
          let response = format(WindowSize {
            num_lines: self.term.screen_lines() as u16,
            num_cols: self.term.columns() as u16,
            cell_width: 0,
            cell_height: 0,
          });
          writer.write_all(response.as_bytes())?;
        }
        Event::ClipboardStore(_, text) => events.clipboard_stores.push(text),
        Event::ClipboardLoad(_, format) => events.clipboard_loads.push(format),
        Event::ColorRequest(_, _)
        | Event::MouseCursorDirty
        | Event::CursorBlinkingChange
        | Event::Wakeup
        | Event::Bell
        | Event::Exit
        | Event::ChildExit(_) => {}
      }
    }
    Ok(events)
  }

  pub fn resize(&mut self, rows: u16, columns: u16) {
    self.term.resize(TerminalSize {
      rows: rows as usize,
      columns: columns as usize,
    });
  }

  pub fn grid(&self) -> &Grid<Cell> { self.term.grid() }

  pub fn snapshot_replay(&self) -> Vec<u8> {
    let mut bytes = b"\x1b[2J\x1b[H".to_vec();
    for row in 0..self.term.screen_lines() {
      for column in 0..self.term.columns() {
        let cell = &self.term.grid()[alacritty_terminal::index::Line(row as i32)]
          [alacritty_terminal::index::Column(column)];
        if !cell
          .flags
          .contains(alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER)
        {
          let mut encoded = [0; 4];
          bytes.extend_from_slice(cell.c.encode_utf8(&mut encoded).as_bytes());
        }
      }
      if row + 1 < self.term.screen_lines() {
        bytes.push(b'\n');
      }
    }
    bytes
  }

  pub fn size(&self) -> (u16, u16) { (self.term.screen_lines() as u16, self.term.columns() as u16) }

  pub fn cursor_visible(&self) -> bool {
    self
      .term
      .mode()
      .contains(alacritty_terminal::term::TermMode::SHOW_CURSOR)
  }
}

#[cfg(test)]
mod tests {
  use super::TerminalEmulator;

  #[test]
  fn terminal_queries_are_answered_by_alacritty() {
    let mut terminal = TerminalEmulator::new(24, 80);
    let mut response = Vec::new();
    terminal
      .process(b"\x1b[c", &mut response)
      .expect("process query");
    assert!(!response.is_empty());
  }

  #[test]
  fn device_status_queries_are_answered() {
    let mut terminal = TerminalEmulator::new(24, 80);
    let mut response = Vec::new();
    terminal
      .process(b"\x1b[5n", &mut response)
      .expect("process query");
    assert!(response.windows(4).any(|bytes| bytes == b"\x1b[0n"));
  }

  #[test]
  fn resize_and_wide_character_geometry_are_preserved() {
    let mut terminal = TerminalEmulator::new(2, 4);
    terminal.resize(3, 8);
    assert_eq!(terminal.size(), (3, 8));
    terminal
      .process("界".as_bytes(), &mut std::io::sink())
      .expect("process text");
    assert!(
      terminal.grid()[alacritty_terminal::index::Line(0)][alacritty_terminal::index::Column(0)]
        .flags
        .contains(alacritty_terminal::term::cell::Flags::WIDE_CHAR)
    );
  }

  #[test]
  fn alternate_screen_is_rendered_by_the_same_terminal() {
    let mut terminal = TerminalEmulator::new(2, 10);
    terminal
      .process(b"main", &mut std::io::sink())
      .expect("process main");
    terminal
      .process(b"\x1b[?1049halt\x1b[?1049l", &mut std::io::sink())
      .expect("process alternate screen");
    assert_eq!(
      terminal.grid()[alacritty_terminal::index::Line(0)][alacritty_terminal::index::Column(0)].c,
      'm'
    );
  }

  #[test]
  fn title_events_are_reported() {
    let mut terminal = TerminalEmulator::new(2, 10);
    let mut response = Vec::new();
    assert_eq!(
      terminal
        .process(b"\x1b]2;editor\x07", &mut response)
        .unwrap()
        .title,
      Some("editor".into())
    );
  }
}
