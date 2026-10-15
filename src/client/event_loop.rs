//! Client-side session orchestration and event loop.
//!
//! This module owns the boundary between the server connection, terminal input,
//! application state, Lua callbacks, and ratatui rendering. [`run`] performs
//! startup and cleanup; [`run_event_loop`] then follows the main cycle:
//!
//! ```text
//! receive input/server event
//!     -> mutate ClientSession and produce Effects
//!     -> dispatch Lua events and runtime commands
//!     -> redraw when Effects::redraw is set
//! ```
//!
//! In simplified form, the ownership of the loop looks like this:
//!
//! ```ignore
//! loop {
//!     if needs_draw {
//!         build_ui_render(&session, &config);
//!         terminal.draw(|frame| ui::draw(frame, &view))?;
//!         needs_draw = false;
//!     }
//!
//!     let event = receiver.recv_timeout(next_timeout)?;
//!     let effects = handle_event(event, &mut context)?;
//!     dispatch_ui_events(effects, &mut context)?;
//! }
//! ```
//!
//! Rendering is deliberately demand-driven. The loop waits on the event
//! channel when no redraw is needed, but periodically wakes to expire key
//! sequences and temporary warnings. Terminal output only requests a redraw
//! when it belongs to the active tab.

use std::{
  collections::{
    HashMap,
    VecDeque,
  },
  io,
  path::{
    Path,
    PathBuf,
  },
  sync::{
    Arc,
    atomic::{
      AtomicBool,
      Ordering,
    },
    mpsc::{
      self,
      Receiver,
      RecvTimeoutError,
      Sender,
    },
  },
  thread,
  time::{
    Duration,
    Instant,
  },
};

use crossterm::{
  cursor::{
    SetCursorStyle,
    Show,
  },
  event::{
    self,
    Event,
    KeyCode,
    KeyEventKind,
    KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
  },
  execute,
  terminal::{
    EnterAlternateScreen,
    LeaveAlternateScreen,
    disable_raw_mode,
    enable_raw_mode,
  },
};
use interprocess::local_socket::prelude::*;
use ratatui::{
  Terminal,
  backend::CrosstermBackend,
  layout::Rect,
};
use zmux::{
  protocol::{
    ClientHello,
    ClientMessage,
    ClientRequest,
    ServerMessage,
    SessionSnapshot,
    TabSnapshot as WireTab,
    TerminalSnapshot,
    Viewport,
    read_frame,
    write_frame,
  },
  transport::{
    self,
    Endpoint,
  },
};

use crate::{
  clipboard::{
    ClipboardProvider,
    SystemClipboard,
  },
  input::{
    KeyDispatcher,
    KeyOutcome,
    KeyTimeout,
    which_key::WhichKeyPopup,
  },
  lua::{
    self,
    AppEvent as UiEvent,
    AppSnapshot,
    LuaConfig,
    RuntimeCommand,
    TabSnapshot,
  },
  terminal_emulator::TerminalEmulator,
  trace::Trace,
  ui,
};

/// Safety bound preventing Lua callbacks from creating an unbounded event loop.
const MAX_CALLBACK_COMMANDS: usize = 128;
const SHUTDOWN_DELAY: Duration = Duration::from_millis(200);

/// Starts the interactive client for a session.
///
/// This function calculates the initial PTY viewport, connects to the server,
/// starts the reader/writer/input worker threads, enters terminal mode, and
/// hands control to [`run_event_loop`]. Terminal mode is restored on every
/// return path through [`TerminalModeGuard`] and the final cursor cleanup.
///
/// `session_name` selects an existing session when present; `None` asks the
/// server to create a new session. Errors from connection, terminal setup, or
/// the event loop are propagated to the caller.
pub fn run(
  endpoint: Endpoint,
  session_name: Option<String>,
  create_if_missing: bool,
  working_directory: PathBuf,
  config: lua::AppConfig,
) -> Result<(), Box<dyn std::error::Error>> {
  let size = crossterm::terminal::size()?;
  let options = config.lua.sync_options()?;
  let pane_size = ui::pane_size(size.0, size.1, options, false);
  let viewport = Viewport {
    rows: pane_size.0.max(1),
    columns: pane_size.1.max(1),
  };
  let connection = connect(
    &endpoint,
    session_name.clone(),
    create_if_missing,
    working_directory,
    viewport,
    config.default_shell.as_deref(),
  )?;
  let (reader, writer) = connection.stream.split();
  let (event_sender, receiver) = mpsc::channel();
  spawn_server_reader(reader, event_sender.clone())?;
  let request_sender = spawn_server_writer(writer, event_sender.clone())?;

  let terminal_mode = TerminalModeGuard::enter(config.cursor_shape)?;
  let trace = Trace::from_environment();
  let backend = CrosstermBackend::new(io::stdout());
  let mut terminal = Terminal::new(backend)?;
  let input_stop = Arc::new(AtomicBool::new(false));
  let input_thread = spawn_input_reader(event_sender, input_stop.clone(), trace.clone())?;
  let result = run_event_loop(
    &mut terminal,
    config,
    receiver,
    request_sender,
    ClientSession::new(connection.session, connection.terminals, viewport),
    connection.client_id,
    session_name.is_none(),
  );

  trace.mark("shutdown-event-loop-returned");
  thread::sleep(SHUTDOWN_DELAY);
  trace.mark("shutdown-delay-complete");
  input_stop.store(true, Ordering::Relaxed);
  let _ = input_thread.join();
  trace.mark("shutdown-input-reader-joined");
  let drained = drain_pending_input(&trace)?;
  trace.mark(&format!("shutdown-pending-input-drained count={drained}"));

  terminal.show_cursor()?;
  drop(terminal);
  drop(terminal_mode);
  result
}

/// RAII guard for raw mode, the alternate screen, cursor shape, and enhanced
/// keyboard reporting.
///
/// Keeping terminal setup and teardown together is important: dropping this
/// guard restores the user's shell even when the event loop returns an error.
struct TerminalModeGuard;

impl TerminalModeGuard {
  /// Enters the terminal mode required by the interactive client.
  fn enter(cursor_shape: ui::CursorShape) -> io::Result<Self> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if let Err(error) = execute!(
      stdout,
      EnterAlternateScreen,
      cursor_shape.terminal_style(),
      PushKeyboardEnhancementFlags(
        KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
          | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
          | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES,
      )
    ) {
      let _ = disable_raw_mode();
      return Err(error);
    }
    Ok(Self)
  }
}

impl Drop for TerminalModeGuard {
  fn drop(&mut self) {
    let mut stdout = io::stdout();
    let _ = execute!(
      stdout,
      PopKeyboardEnhancementFlags,
      SetCursorStyle::DefaultUserShape,
      LeaveAlternateScreen,
      Show
    );
    let _ = disable_raw_mode();
  }
}

/// State returned by the server handshake.
struct InitialConnection {
  /// Connected stream retained for splitting into reader and writer halves.
  stream: interprocess::local_socket::prelude::LocalSocketStream,
  /// Client identity assigned by the server.
  client_id: u64,
  /// Initial session snapshot.
  session: SessionSnapshot,
  /// Initial terminal snapshots for the session's tabs.
  terminals: Vec<TerminalSnapshot>,
}

/// Connects to the server and completes the client handshake.
///
/// The client sends its protocol version, optional session target, working
/// directory, and PTY viewport. It then waits for both acceptance and the
/// initial session state before returning.
fn current_shell(configured_shell: Option<&Path>) -> PathBuf {
  if let Some(shell) = configured_shell {
    return shell.to_owned();
  }
  #[cfg(windows)]
  {
    std::env::var_os("COMSPEC")
      .map(PathBuf::from)
      .unwrap_or_else(|| PathBuf::from("cmd.exe"))
  }
  #[cfg(not(windows))]
  {
    std::env::var_os("SHELL")
      .map(PathBuf::from)
      .unwrap_or_else(|| PathBuf::from("sh"))
  }
}

fn connect(
  endpoint: &Endpoint,
  session_name: Option<String>,
  create_if_missing: bool,
  working_directory: PathBuf,
  viewport: Viewport,
  configured_shell: Option<&Path>,
) -> Result<InitialConnection, Box<dyn std::error::Error>> {
  let mut stream = transport::connect(endpoint)?;
  write_frame(
    &mut stream,
    &ClientMessage::Hello(ClientHello {
      protocol_version: zmux::protocol::PROTOCOL_VERSION,
      session_name,
      create_if_missing,
      shell: current_shell(configured_shell),
      environment: std::env::vars().collect(),
      working_directory,
      viewport,
    }),
  )?;
  let mut client_id = None;
  loop {
    match read_frame::<_, ServerMessage>(&mut stream)? {
      ServerMessage::HelloAccepted { client_id: id, .. } => client_id = Some(id),
      ServerMessage::HelloRejected { reason } => return Err(reason.into()),
      ServerMessage::InitialState { session, terminals } => {
        let client_id = client_id.ok_or("server sent state before accepting the Client")?;
        return Ok(InitialConnection {
          stream,
          client_id,
          session,
          terminals,
        });
      }
      message => {
        return Err(format!("unexpected handshake message: {message:?}").into());
      }
    }
  }
}

/// Client-side terminal state for one tab.
///
/// The emulator owns the current terminal grid used by [`ui::draw`]. The title is
/// copied separately because terminal output may update it incrementally.
struct TerminalView {
  /// Alacritty terminal emulator and screen buffer.
  emulator: TerminalEmulator,
  /// Most recently observed terminal title.
  title: String,
  /// Process exit status, if the process has exited.
  exit_code: Option<u32>,
}

impl TerminalView {
  /// Builds a parser and replays the server's terminal snapshot into it.
  fn from_snapshot(snapshot: TerminalSnapshot) -> Self {
    let mut emulator = TerminalEmulator::new(snapshot.rows, snapshot.columns);
    let _ = emulator.process(&snapshot.bytes, &mut io::sink());
    let fallback_title = format!("Terminal {}", snapshot.tab_id);
    let title = if snapshot.title.is_empty() {
      fallback_title.clone()
    } else {
      snapshot.title
    };
    Self {
      emulator,
      title,
      exit_code: snapshot.exit_code,
    }
  }
}

/// Whether a workspace dialog creates a workspace or edits an existing one.
#[derive(Clone, Copy)]
enum WorkspaceDialogMode {
  /// Create a new workspace.
  Create,
  /// Edit the workspace with this stable ID.
  Edit(u64),
}

/// Field currently focused in the workspace form.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkspaceField {
  /// Working-directory field.
  Directory,
  /// Optional workspace-name field.
  Name,
}

/// Mutable state for the create/edit workspace form.
struct WorkspaceDialog {
  /// Whether submission creates or edits.
  mode: WorkspaceDialogMode,
  /// Directory selected for the workspace.
  working_directory: PathBuf,
  /// Optional explicit workspace name.
  name: String,
  /// Form field currently selected for editing.
  field: WorkspaceField,
  /// Whether the selected field is accepting text input.
  editing: bool,
}

/// One row in the working-directory picker.
struct DirectoryEntry {
  /// Display text used by fuzzy filtering and the modal.
  label: String,
  /// Directory navigated to or selected.
  path: PathBuf,
  /// Whether Enter selects this directory instead of opening it.
  use_current: bool,
}

/// Navigable, fuzzy-filtered directory picker nested inside a workspace form.
struct DirectoryPicker {
  /// Form restored when the picker closes.
  form: WorkspaceDialog,
  /// Directory whose children are currently listed.
  directory: PathBuf,
  /// Current fuzzy-search query.
  query: String,
  /// Entries in the unfiltered list.
  entries: Vec<DirectoryEntry>,
  /// Index within the filtered results.
  highlighted: usize,
}

/// Active workspace-related modal.
enum WorkspaceModal {
  /// Create/edit form.
  Form(WorkspaceDialog),
  /// Directory selection view opened from the form.
  Directory(DirectoryPicker),
}

/// Local state for a picker requested by a Lua callback.
struct LuaPickerState {
  /// Identifier used to complete the picker in Lua.
  id: u64,
  /// Candidate labels supplied by Lua.
  labels: Vec<String>,
  /// Current fuzzy-search query.
  query: String,
  /// Index within the filtered results.
  highlighted: usize,
}

/// Owned, render-ready representation of any client modal.
struct ModalDisplay {
  /// Border title.
  title: String,
  /// Optional filter query.
  query: Option<String>,
  /// Rows displayed by the modal renderer.
  rows: Vec<String>,
  /// Selected row in display coordinates.
  selected_row: Option<usize>,
  /// Keyboard help or status footer.
  footer: String,
}

/// All client-owned state needed to process events and build a UI frame.
///
/// The server remains authoritative for session/workspace/tab structure;
/// terminal parsers, modal state, warnings, and redraw flags are local client
/// concerns.
struct ClientSession {
  snapshot: SessionSnapshot,
  terminals: HashMap<u64, TerminalView>,
  viewport: Viewport,
  last_error: Option<String>,
  key_warning: Option<(String, Instant)>,
  drawer_expanded: bool,
  drawer_highlighted_workspace: Option<u64>,
  workspace_modal: Option<WorkspaceModal>,
  lua_picker: Option<LuaPickerState>,
  which_key_scroll: usize,
  which_key_options: crate::lua::WhichKeyOptions,
  pending_shutdown: Option<(KeyCode, Vec<RuntimeCommand>)>,
}

impl ClientSession {
  /// Creates client state from the server's initial snapshot.
  fn new(snapshot: SessionSnapshot, terminals: Vec<TerminalSnapshot>, viewport: Viewport) -> Self {
    Self {
      snapshot,
      terminals: terminals
        .into_iter()
        .map(|terminal| {
          let id = terminal.tab_id;
          (id, TerminalView::from_snapshot(terminal))
        })
        .collect(),
      viewport,
      last_error: None,
      key_warning: None,
      drawer_expanded: false,
      drawer_highlighted_workspace: None,
      workspace_modal: None,
      lua_picker: None,
      which_key_scroll: 0,
      which_key_options: crate::lua::WhichKeyOptions::default(),
      pending_shutdown: None,
    }
  }

  /// Returns the workspace selected by the session snapshot.
  fn active_workspace(&self) -> Option<&zmux::protocol::WorkspaceSnapshot> {
    self
      .snapshot
      .workspaces
      .iter()
      .find(|workspace| workspace.id == self.snapshot.active_workspace_id)
  }

  /// Returns the active tab within the active workspace.
  fn active_tab(&self) -> Option<&WireTab> {
    let workspace = self.active_workspace()?;
    workspace
      .tabs
      .iter()
      .find(|tab| tab.id == workspace.active_tab_id)
  }

  /// Returns the active tab's zero-based index for UI selection.
  fn active_tab_index(&self) -> usize {
    self
      .active_workspace()
      .and_then(|workspace| {
        workspace
          .tabs
          .iter()
          .position(|tab| tab.id == workspace.active_tab_id)
      })
      .unwrap_or_default()
  }

  /// Returns the parsed terminal belonging to the active tab.
  fn active_terminal(&self) -> Option<&TerminalView> { self.terminals.get(&self.active_tab()?.id) }

  /// Replaces server state and rebuilds all terminal parsers.
  ///
  /// A missing highlighted workspace is reset to the new active workspace so
  /// drawer navigation cannot retain a deleted ID.
  fn set_snapshot(&mut self, snapshot: SessionSnapshot, terminals: Vec<TerminalSnapshot>) {
    self.snapshot = snapshot;
    if self.drawer_highlighted_workspace.is_none_or(|id| {
      !self
        .snapshot
        .workspaces
        .iter()
        .any(|workspace| workspace.id == id)
    }) {
      self.drawer_highlighted_workspace = Some(self.snapshot.active_workspace_id);
    }
    self.terminals = terminals
      .into_iter()
      .map(|terminal| {
        let id = terminal.tab_id;
        (id, TerminalView::from_snapshot(terminal))
      })
      .collect();
  }

  /// Feeds terminal output into a tab's Alacritty emulator.
  fn process_output(&mut self, tab_id: u64, bytes: &[u8]) -> Vec<Vec<u8>> {
    let Some(terminal) = self.terminals.get_mut(&tab_id) else {
      return Vec::new();
    };
    let events = match terminal.emulator.process(bytes, &mut io::sink()) {
      Ok(events) => events,
      Err(_) => return Vec::new(),
    };
    let clipboard = SystemClipboard;
    events
      .clipboard_loads
      .into_iter()
      .filter_map(|format| clipboard.read().ok().map(|text| format(&text).into_bytes()))
      .collect()
  }

  /// Builds the public application snapshot exposed to Lua callbacks.
  ///
  /// Tab indices are one-based for configuration-facing APIs, while the
  /// internal active-tab index remains zero-based.
  fn snapshot_for_lua(
    &self,
    leader_mode: bool,
    terminal_width: u16,
    terminal_height: u16,
  ) -> AppSnapshot {
    let tabs: Vec<TabSnapshot> = self
      .active_workspace()
      .into_iter()
      .flat_map(|workspace| workspace.tabs.iter().enumerate())
      .map(|(index, tab)| {
        let exit_code = self.terminals.get(&tab.id).and_then(|view| view.exit_code);
        TabSnapshot {
          id: tab.id,
          index: index + 1,
          title: self
            .terminals
            .get(&tab.id)
            .map_or_else(|| tab.title.clone(), |view| view.title.clone()),
          selected: self.active_tab().is_some_and(|active| active.id == tab.id),
          exit_code,
        }
      })
      .collect();
    AppSnapshot {
      session: Some(self.snapshot.clone()),
      last_error: self.last_error.clone(),
      tabs,
      active_tab: self.active_workspace().and_then(|workspace| {
        workspace
          .tabs
          .iter()
          .position(|tab| tab.id == workspace.active_tab_id)
      }),
      leader_mode,
      terminal_rows: terminal_height,
      terminal_columns: terminal_width,
    }
  }

  /// Stores the viewport most recently sent to the server.
  fn resize(&mut self, viewport: Viewport) { self.viewport = viewport; }

  /// Opens a create-workspace form seeded from the active directory.
  fn open_create_workspace(&mut self) -> Result<(), Box<dyn std::error::Error>> {
    let working_directory = self
      .active_workspace()
      .map(|workspace| workspace.working_directory.clone())
      .map_or_else(std::env::current_dir, Ok)?;
    self.workspace_modal = Some(WorkspaceModal::Form(WorkspaceDialog {
      mode: WorkspaceDialogMode::Create,
      working_directory,
      name: String::new(),
      field: WorkspaceField::Directory,
      editing: false,
    }));
    Ok(())
  }

  /// Opens an edit-workspace form for the active workspace.
  fn open_edit_workspace(&mut self) -> Result<(), Box<dyn std::error::Error>> {
    let workspace = self
      .active_workspace()
      .ok_or("Session has no active Workspace")?;
    self.workspace_modal = Some(WorkspaceModal::Form(WorkspaceDialog {
      mode: WorkspaceDialogMode::Edit(workspace.id),
      working_directory: workspace.working_directory.clone(),
      name: workspace.explicit_name.clone().unwrap_or_default(),
      field: WorkspaceField::Directory,
      editing: false,
    }));
    Ok(())
  }

  /// Converts the active picker or workspace modal into render data.
  fn modal_display(&self) -> Option<ModalDisplay> {
    if let Some(picker) = &self.lua_picker {
      let visible = zmux::picker::fuzzy_filter(&picker.query, &picker.labels);
      return Some(ModalDisplay {
        title: "Select an item".to_owned(),
        query: Some(picker.query.clone()),
        rows: visible
          .iter()
          .map(|index| picker.labels[*index].clone())
          .collect(),
        selected_row: (!visible.is_empty()).then_some(picker.highlighted),
        footer: "Arrows: move  Enter: select  Esc: cancel".to_owned(),
      });
    }
    match self.workspace_modal.as_ref()? {
      WorkspaceModal::Form(form) => {
        let directory_name = form
          .working_directory
          .file_name()
          .map(|name| name.to_string_lossy().into_owned())
          .unwrap_or_else(|| form.working_directory.to_string_lossy().into_owned());
        let name = if form.name.is_empty() {
          format!("(automatic: {directory_name})")
        } else {
          form.name.clone()
        };
        let title = match form.mode {
          WorkspaceDialogMode::Create => "Create Workspace",
          WorkspaceDialogMode::Edit(_) => "Edit Workspace",
        };
        Some(ModalDisplay {
          title: title.to_owned(),
          query: None,
          rows: vec![
            format!("Directory: {}", form.working_directory.display()),
            format!("Name (optional): {name}"),
          ],
          selected_row: Some(match form.field {
            WorkspaceField::Directory => 0,
            WorkspaceField::Name => 1,
          }),
          footer: "Tab: next  Shift+Tab: previous  e: edit  Enter: submit  Esc: cancel".to_owned(),
        })
      }
      WorkspaceModal::Directory(picker) => {
        let visible = picker.visible_indices();
        Some(ModalDisplay {
          title: "Choose Working Directory".to_owned(),
          query: Some(picker.query.clone()),
          rows: visible
            .iter()
            .map(|index| picker.entries[*index].label.clone())
            .collect(),
          selected_row: (!visible.is_empty()).then_some(picker.highlighted),
          footer: "Arrows: move  Enter: open/use  Esc: back".to_owned(),
        })
      }
    }
  }
}

impl DirectoryPicker {
  /// Opens a picker for the form's current directory and loads its entries.
  fn new(form: WorkspaceDialog) -> io::Result<Self> {
    let directory = form.working_directory.clone();
    let mut picker = Self {
      form,
      directory,
      query: String::new(),
      entries: Vec::new(),
      highlighted: 0,
    };
    picker.reload()?;
    Ok(picker)
  }

  /// Rebuilds the current-directory, parent, and child-directory entries.
  ///
  /// Children are sorted case-insensitively and the highlight is reset to the
  /// first result after navigation.
  fn reload(&mut self) -> io::Result<()> {
    let mut entries = vec![DirectoryEntry {
      label: format!("Use this directory: {}", self.directory.display()),
      path: self.directory.clone(),
      use_current: true,
    }];
    if let Some(parent) = self.directory.parent()
      && parent != self.directory
    {
      entries.push(DirectoryEntry {
        label: "../".to_owned(),
        path: parent.to_path_buf(),
        use_current: false,
      });
    }
    let mut children = Vec::new();
    for entry in std::fs::read_dir(&self.directory)? {
      let entry = entry?;
      if entry.file_type()?.is_dir() {
        let label = entry.file_name().to_string_lossy().into_owned();
        children.push(DirectoryEntry {
          label,
          path: entry.path(),
          use_current: false,
        });
      }
    }
    children.sort_by_key(|entry| entry.label.to_lowercase());
    entries.extend(children);
    self.entries = entries;
    self.highlighted = 0;
    Ok(())
  }

  /// Returns source entry indices matching the current query.
  fn visible_indices(&self) -> Vec<usize> {
    let labels = self
      .entries
      .iter()
      .map(|entry| entry.label.clone())
      .collect::<Vec<_>>();
    zmux::picker::fuzzy_filter(&self.query, &labels)
  }

  /// Moves the filtered-result highlight, wrapping at either end.
  fn move_highlight(&mut self, direction: i32) {
    let count = self.visible_indices().len();
    if count == 0 {
      self.highlighted = 0;
    } else if direction < 0 {
      self.highlighted = if self.highlighted == 0 {
        count - 1
      } else {
        self.highlighted - 1
      };
    } else {
      self.highlighted = (self.highlighted + 1) % count;
    }
  }

  /// Confirms the highlighted entry.
  ///
  /// Selecting “use this directory” returns a path. Selecting another entry
  /// navigates into it, reloads the picker, and returns `None`.
  fn confirm(&mut self) -> io::Result<Option<PathBuf>> {
    let visible = self.visible_indices();
    let Some(entry_index) = visible.get(self.highlighted).copied() else {
      return Ok(None);
    };
    let entry = &self.entries[entry_index];
    if entry.use_current {
      Ok(Some(entry.path.clone()))
    } else {
      self.directory = entry.path.clone();
      self.query.clear();
      self.reload()?;
      Ok(None)
    }
  }
}

/// Accumulated consequences of handling one input or server event.
///
/// Effects separate state transitions from the event loop: handlers can ask
/// for a redraw, queue Lua events, or terminate without directly controlling
/// the loop.
#[derive(Default)]
struct Effects {
  /// Stop the client loop.
  quit: bool,
  /// Request another ratatui frame.
  redraw: bool,
  /// Application events to deliver to Lua handlers.
  events: Vec<UiEvent>,
  /// Terminal responses that must be forwarded to the server PTY.
  inputs: Vec<(u64, Vec<u8>)>,
}

impl Effects {
  /// Combines flags and appends pending events in their existing order.
  fn merge(&mut self, mut other: Self) {
    self.quit |= other.quit;
    self.redraw |= other.redraw;
    self.events.append(&mut other.events);
    self.inputs.append(&mut other.inputs);
  }
}

/// Mutable services shared by event handlers and runtime-command execution.
struct ClientContext<'a> {
  /// Mutable application/session state.
  session: &'a mut ClientSession,
  /// Current outer terminal size as `(width, height)`.
  outer_size: &'a mut (u16, u16),
  /// PTY viewport currently known by the client and server.
  viewport: &'a mut Viewport,
  /// Prefix/sequence key state machine.
  dispatcher: &'a mut KeyDispatcher,
  /// Outgoing server-request channel.
  sender: &'a Sender<ClientMessage>,
  /// Next request ID to assign.
  next_request_id: &'a mut u64,
  /// Lua configuration and callback interface.
  lua: &'a LuaConfig,
  /// Trace sink for event and performance diagnostics.
  trace: &'a Trace,
}

/// Messages delivered to the main client loop.
enum ClientEvent {
  /// Keyboard, resize, or other terminal input.
  Input(Event),
  /// Message received from the server reader thread.
  Server(ServerMessage),
}

/// Creates the synthetic lifecycle events emitted after the initial handshake.
///
/// A newly created session receives creation events for its workspace and tabs;
/// every session receives selection events for the active workspace and tab.
fn initial_session_events(session: &SessionSnapshot, created: bool) -> Vec<UiEvent> {
  let mut events = vec![UiEvent {
    name: if created {
      "session_created"
    } else {
      "session_selected"
    }
    .to_owned(),
    session_number: Some(session.number.clone()),
    ..UiEvent::default()
  }];
  let Some(active_workspace) = session
    .workspaces
    .iter()
    .find(|workspace| workspace.id == session.active_workspace_id)
  else {
    return events;
  };
  let workspace_index = session
    .workspaces
    .iter()
    .position(|workspace| workspace.id == active_workspace.id)
    .map(|index| index + 1);
  if created {
    events.push(UiEvent {
      name: "workspace_created".to_owned(),
      session_number: Some(session.number.clone()),
      workspace_id: Some(active_workspace.id),
      workspace_index,
      ..UiEvent::default()
    });
    for (index, tab) in active_workspace.tabs.iter().enumerate() {
      events.push(UiEvent {
        name: "tab_created".to_owned(),
        session_number: Some(session.number.clone()),
        workspace_id: Some(active_workspace.id),
        workspace_index,
        tab_id: Some(tab.id),
        index: Some(index + 1),
        ..UiEvent::default()
      });
    }
  }
  events.push(UiEvent {
    name: "workspace_selected".to_owned(),
    session_number: Some(session.number.clone()),
    workspace_id: Some(active_workspace.id),
    workspace_index,
    ..UiEvent::default()
  });
  if let Some((index, tab)) = active_workspace
    .tabs
    .iter()
    .enumerate()
    .find(|(_, tab)| tab.id == active_workspace.active_tab_id)
  {
    events.push(UiEvent {
      name: "tab_selected".to_owned(),
      session_number: Some(session.number.clone()),
      workspace_id: Some(active_workspace.id),
      workspace_index,
      tab_id: Some(tab.id),
      index: Some(index + 1),
      ..UiEvent::default()
    });
  }
  events
}

/// Runs the demand-driven client event and render loop.
///
/// The loop renders once when `needs_draw` is set, then waits for input or
/// server messages. Each handler returns [`Effects`], which determines whether
/// state changed and whether another frame is needed. A bounded timeout also
/// lets the loop expire key sequences and temporary warnings.
///
/// The render phase assembles a [`ui::UiRender`] from the client session and
/// invokes `ratatui`'s `Terminal::draw`; event handling resumes only after the
/// frame is complete.
fn run_event_loop(
  terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
  config: lua::AppConfig,
  receiver: Receiver<ClientEvent>,
  sender: Sender<ClientMessage>,
  mut session: ClientSession,
  client_id: u64,
  created_session: bool,
) -> Result<(), Box<dyn std::error::Error>> {
  let trace = Trace::from_environment();
  trace.mark("client-attached");
  let size = terminal.size()?;
  let mut outer_size = (size.width, size.height);
  let mut ui_options = config.lua.sync_options()?;
  let initial_pane = ui::pane_size(outer_size.0, outer_size.1, ui_options, session.drawer_expanded);
  let mut viewport = Viewport {
    rows: initial_pane.0.max(1),
    columns: initial_pane.1.max(1),
  };
  let mut next_request_id = 1;
  let mut dispatcher =
    KeyDispatcher::new(config.prefix_key, config.sequence_timeout, config.keymap);
  let initial_events = initial_session_events(&session.snapshot, created_session);
  let startup = dispatch_ui_events(
    Effects {
      events: initial_events,
      ..Effects::default()
    },
    &mut ClientContext {
      session: &mut session,
      outer_size: &mut outer_size,
      viewport: &mut viewport,
      dispatcher: &mut dispatcher,
      sender: &sender,
      next_request_id: &mut next_request_id,
      lua: &config.lua,
      trace: &trace,
    },
  )?;
  if startup.quit {
    trace.flush();
    return Ok(());
  }
  let mut needs_draw = true;
  let mut last_draw = Instant::now();

  'event_loop: loop {
    if needs_draw {
      let size = terminal.size()?;
      outer_size = (size.width, size.height);
      ui_options = config.lua.sync_options()?;
      session.which_key_options = ui_options.which;
      let pane = ui::pane_size(outer_size.0, outer_size.1, ui_options, session.drawer_expanded);
      let new_viewport = Viewport {
        rows: pane.0.max(1),
        columns: pane.1.max(1),
      };
      if new_viewport != viewport {
        viewport = new_viewport;
        session.resize(viewport);
        send_request(&sender, &mut next_request_id, ClientRequest::Resize(viewport))?;
      }
      config.lua.sync_state(&session.snapshot_for_lua(
        dispatcher.is_leader(),
        outer_size.0,
        outer_size.1,
      ))?;
      let bars = render_bars(&config.lua)?;
      let active_terminal = session
        .active_terminal()
        .ok_or("active Tab has no terminal snapshot")?;
      let drawer_items: Vec<_> = session
        .snapshot
        .workspaces
        .iter()
        .map(|workspace| ui::WorkspaceDrawerItem {
          id: workspace.id,
          name: &workspace.name,
        })
        .collect();
      let drawer = ui::WorkspaceDrawerRender {
        expanded: session.drawer_expanded,
        active_id: session.snapshot.active_workspace_id,
        highlighted_id: session
          .drawer_highlighted_workspace
          .unwrap_or(session.snapshot.active_workspace_id),
        items: &drawer_items,
      };
      let modal_display = session.modal_display();
      let modal = modal_display.as_ref().map(|display| ui::ModalOverlay {
        title: &display.title,
        query: display.query.as_deref(),
        rows: &display.rows,
        selected_row: display.selected_row,
        footer: &display.footer,
      });
      let key_warning = session
        .key_warning
        .as_ref()
        .filter(|(_, deadline)| Instant::now() < *deadline)
        .map(|(warning, _)| warning.as_str());
      let ui_frame = ui::UiRender {
        screen: active_terminal.emulator.grid(),
        cursor_visible: active_terminal.emulator.cursor_visible(),
        leader_mode: dispatcher.is_leader(),
        tab_count: session
          .active_workspace()
          .map_or(0, |workspace| workspace.tabs.len()),
        active_tab: session.active_tab_index(),
        active_title: session
          .last_error
          .as_deref()
          .unwrap_or(active_terminal.title.as_str()),
        active_exit_code: active_terminal.exit_code,
        workspace_drawer: drawer,
        modal,
        which_key: WhichKeyPopup {
          prefix: dispatcher.sequence().to_vec(),
          completions: if key_warning.is_some() {
            Vec::new()
          } else {
            dispatcher.completions()
          },
          scroll_offset: session.which_key_scroll,
          options: ui_options.which,
        },
        key_warning,
        options: ui_options,
        bars,
        lua: &config.lua,
        trace: &trace,
      };
      let mut draw_error = None;
      terminal.draw(|frame| {
        if let Err(error) = ui::draw(frame, &ui_frame) {
          draw_error = Some(error);
        }
      })?;
      if let Some(error) = draw_error {
        return Err(error.into());
      }
      needs_draw = false;
      last_draw = Instant::now();
    }

    let timeout = dispatcher
      .time_until_timeout(Instant::now())
      .unwrap_or(Duration::from_millis(250))
      .min(Duration::from_millis(250));
    match receiver.recv_timeout(timeout) {
      Ok(ClientEvent::Input(event)) => {
        let effects = {
          let mut context = ClientContext {
            session: &mut session,
            outer_size: &mut outer_size,
            viewport: &mut viewport,
            dispatcher: &mut dispatcher,
            sender: &sender,
            next_request_id: &mut next_request_id,
            lua: &config.lua,
            trace: &trace,
          };
          handle_input(event, &mut context)?
        };
        needs_draw |= effects.redraw;
        if effects.quit {
          break 'event_loop;
        }
        let effects = dispatch_ui_events(effects, &mut ClientContext {
          session: &mut session,
          outer_size: &mut outer_size,
          viewport: &mut viewport,
          dispatcher: &mut dispatcher,
          sender: &sender,
          next_request_id: &mut next_request_id,
          lua: &config.lua,
          trace: &trace,
        })?;
        needs_draw |= effects.redraw;
        if effects.quit {
          break 'event_loop;
        }
      }
      Ok(ClientEvent::Server(message)) => {
        let mut effects = handle_server_message(message, &mut session);
        for (tab_id, input) in &effects.inputs {
          let _ = tab_id;
          send_request(&sender, &mut next_request_id, ClientRequest::Input(input.clone()))?;
        }
        if !effects.events.is_empty() {
          effects = dispatch_ui_events(effects, &mut ClientContext {
            session: &mut session,
            outer_size: &mut outer_size,
            viewport: &mut viewport,
            dispatcher: &mut dispatcher,
            sender: &sender,
            next_request_id: &mut next_request_id,
            lua: &config.lua,
            trace: &trace,
          })?;
        }
        needs_draw |= effects.redraw;
        if effects.quit {
          break 'event_loop;
        }
      }
      Err(RecvTimeoutError::Timeout) => {
        let now = Instant::now();
        if let Some(timeout) = dispatcher.expire(now) {
          match timeout {
            KeyTimeout::Execute(callback) => {
              let mut effects = match config.lua.invoke_key_callback(&callback) {
                Ok(commands) => execute_runtime_commands(commands, &mut ClientContext {
                  session: &mut session,
                  outer_size: &mut outer_size,
                  viewport: &mut viewport,
                  dispatcher: &mut dispatcher,
                  sender: &sender,
                  next_request_id: &mut next_request_id,
                  lua: &config.lua,
                  trace: &trace,
                })?,
                Err(error) => {
                  session.key_warning = Some((
                    format!("Key callback: {error}"),
                    Instant::now() + Duration::from_secs(3),
                  ));
                  Effects::default()
                }
              };
              effects.redraw = true;
              effects.events.push(UiEvent {
                name: "leader_mode_changed".to_owned(),
                active: Some(false),
                ..UiEvent::default()
              });
              let effects = dispatch_ui_events(effects, &mut ClientContext {
                session: &mut session,
                outer_size: &mut outer_size,
                viewport: &mut viewport,
                dispatcher: &mut dispatcher,
                sender: &sender,
                next_request_id: &mut next_request_id,
                lua: &config.lua,
                trace: &trace,
              })?;
              if effects.quit {
                break 'event_loop;
              }
              needs_draw |= effects.redraw;
            }
            KeyTimeout::LeaderExpired => {
              needs_draw = true;
              let effects = dispatch_ui_events(
                Effects {
                  redraw: true,
                  events: vec![UiEvent {
                    name: "leader_mode_changed".to_owned(),
                    active: Some(false),
                    ..UiEvent::default()
                  }],
                  ..Effects::default()
                },
                &mut ClientContext {
                  session: &mut session,
                  outer_size: &mut outer_size,
                  viewport: &mut viewport,
                  dispatcher: &mut dispatcher,
                  sender: &sender,
                  next_request_id: &mut next_request_id,
                  lua: &config.lua,
                  trace: &trace,
                },
              )?;
              needs_draw |= effects.redraw;
            }
          }
        }
        if session
          .key_warning
          .as_ref()
          .is_some_and(|(_, deadline)| now >= *deadline)
        {
          session.key_warning = None;
          needs_draw = true;
        }
        if last_draw.elapsed() > Duration::from_secs(60) {
          needs_draw = true;
        }
      }
      Err(RecvTimeoutError::Disconnected) => break,
    }
  }
  trace.mark(&format!("client-detached-{client_id}"));
  trace.flush();
  Ok(())
}

/// Starts the thread that decodes server messages and forwards them to the
/// main event loop. A read failure is represented as `ServerStopping` so the
/// loop can shut down through its normal event path.
fn spawn_server_reader(
  mut reader: impl io::Read + Send + 'static,
  sender: Sender<ClientEvent>,
) -> io::Result<()> {
  thread::Builder::new()
    .name("zmux-server-reader".to_owned())
    .spawn(move || {
      loop {
        match read_frame::<_, ServerMessage>(&mut reader) {
          Ok(message) => {
            if sender.send(ClientEvent::Server(message)).is_err() {
              break;
            }
          }
          Err(_) => {
            let _ = sender.send(ClientEvent::Server(ServerMessage::ServerStopping));
            break;
          }
        }
      }
    })?;
  Ok(())
}

/// Starts the thread that serializes outgoing client messages.
///
/// The returned sender is non-blocking with respect to socket writes from the
/// event loop; the writer thread reports a failed connection as
/// `ServerStopping`.
fn spawn_server_writer(
  mut writer: impl io::Write + Send + 'static,
  sender: Sender<ClientEvent>,
) -> io::Result<Sender<ClientMessage>> {
  let (outgoing, receiver) = mpsc::channel::<ClientMessage>();
  thread::Builder::new()
    .name("zmux-server-writer".to_owned())
    .spawn(move || {
      while let Ok(message) = receiver.recv() {
        if write_frame(&mut writer, &message).is_err() {
          let _ = sender.send(ClientEvent::Server(ServerMessage::ServerStopping));
          break;
        }
      }
    })?;
  Ok(outgoing)
}

/// Starts the thread that reads crossterm events and forwards them to the loop.
fn drain_pending_input(trace: &Trace) -> io::Result<usize> {
  // Give bytes already emitted by the terminal a short opportunity to arrive,
  // then consume everything currently pending while raw mode is still active.
  let mut count = 0;
  while event::poll(Duration::from_millis(10))? {
    let event = event::read()?;
    trace.mark(&format!("shutdown-drained-event {event:?}"));
    count += 1;
    while event::poll(Duration::ZERO)? {
      let event = event::read()?;
      trace.mark(&format!("shutdown-drained-event {event:?}"));
      count += 1;
    }
  }
  Ok(count)
}

fn spawn_input_reader(
  sender: Sender<ClientEvent>,
  stop: Arc<AtomicBool>,
  trace: Trace,
) -> io::Result<thread::JoinHandle<()>> {
  thread::Builder::new()
    .name("zmux-input-reader".to_owned())
    .spawn(move || {
      while !stop.load(Ordering::Relaxed) {
        match event::poll(Duration::from_millis(50)) {
          Ok(true) => match event::read() {
            Ok(event) if !stop.load(Ordering::Relaxed) => {
              trace.mark(&format!("input-reader-event {event:?}"));
              if sender.send(ClientEvent::Input(event)).is_err() {
                break;
              }
            }
            Ok(event) => {
              trace.mark(&format!("input-reader-dropped-after-stop {event:?}"));
            }
            Err(error) => {
              trace.mark(&format!("input-reader-error {error}"));
              break;
            }
          },
          Ok(false) => {}
          Err(_) => break,
        }
      }
    })
}

/// Applies one server message to local state and returns its UI effects.
///
/// Active terminal output redraws immediately; output for an inactive tab is
/// still parsed but waits until that tab becomes visible. State changes and
/// replies redraw, while detach/stop messages terminate the client.
fn handle_server_message(message: ServerMessage, session: &mut ClientSession) -> Effects {
  match message {
    ServerMessage::StateChanged {
      session: snapshot,
      terminals,
    } => {
      let events = diff_session_state(&session.snapshot, &snapshot);
      session.set_snapshot(snapshot, terminals);
      session.last_error = None;
      Effects {
        redraw: true,
        events,
        ..Effects::default()
      }
    }
    ServerMessage::ClipboardStore { text, .. } => {
      if let Err(error) = SystemClipboard.write(&text) {
        session.last_error = Some(format!("clipboard: {error}"));
      }
      Effects {
        redraw: true,
        ..Effects::default()
      }
    }
    ServerMessage::TerminalOutput { tab_id, bytes } => {
      let active = session.active_tab().is_some_and(|tab| tab.id == tab_id);
      let inputs = session.process_output(tab_id, &bytes);
      Effects {
        redraw: active,
        inputs: inputs.into_iter().map(|input| (tab_id, input)).collect(),
        ..Effects::default()
      }
    }
    ServerMessage::TerminalChanged(snapshot) => {
      session
        .terminals
        .insert(snapshot.tab_id, TerminalView::from_snapshot(snapshot));
      Effects {
        redraw: true,
        ..Effects::default()
      }
    }
    ServerMessage::Reply {
      result: Err(error), ..
    } => {
      session.last_error = Some(error);
      Effects {
        redraw: true,
        ..Effects::default()
      }
    }
    ServerMessage::Reply { result: Ok(()), .. } => {
      session.last_error = None;
      Effects {
        redraw: true,
        ..Effects::default()
      }
    }
    ServerMessage::SessionClosed => Effects {
      quit: true,
      events: vec![UiEvent {
        name: "session_closed".to_owned(),
        session_number: Some(session.snapshot.number.clone()),
        ..UiEvent::default()
      }],
      ..Effects::default()
    },
    ServerMessage::Detached | ServerMessage::ServerStopping => Effects {
      quit: true,
      ..Effects::default()
    },
    ServerMessage::HelloAccepted { .. }
    | ServerMessage::HelloRejected { .. }
    | ServerMessage::InitialState { .. }
    | ServerMessage::AdminResult { .. } => Effects::default(),
  }
}

/// Computes Lua lifecycle events between two server snapshots.
///
/// The diff covers sessions, workspaces, tabs, titles, process exits, and
/// active workspace/tab changes. It compares stable IDs rather than positions
/// so reordering does not look like deletion and recreation.
fn diff_session_state(old: &SessionSnapshot, new: &SessionSnapshot) -> Vec<UiEvent> {
  let mut events = Vec::new();
  if old.number != new.number {
    events.push(UiEvent {
      name: "session_closed".to_owned(),
      session_number: Some(old.number.clone()),
      ..UiEvent::default()
    });
    events.push(UiEvent {
      name: "session_selected".to_owned(),
      session_number: Some(new.number.clone()),
      ..UiEvent::default()
    });
  }
  for (index, workspace) in new.workspaces.iter().enumerate() {
    if !old
      .workspaces
      .iter()
      .any(|previous| previous.id == workspace.id)
    {
      events.push(UiEvent {
        name: "workspace_created".to_owned(),
        session_number: Some(new.number.clone()),
        workspace_id: Some(workspace.id),
        workspace_index: Some(index + 1),
        ..UiEvent::default()
      });
    }
  }
  for (index, workspace) in old.workspaces.iter().enumerate() {
    if !new
      .workspaces
      .iter()
      .any(|current| current.id == workspace.id)
    {
      events.push(UiEvent {
        name: "workspace_closed".to_owned(),
        session_number: Some(new.number.clone()),
        workspace_id: Some(workspace.id),
        workspace_index: Some(index + 1),
        ..UiEvent::default()
      });
    }
  }

  for (workspace_index, workspace) in new.workspaces.iter().enumerate() {
    for (tab_index, tab) in workspace.tabs.iter().enumerate() {
      let previous = old
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.tabs.iter())
        .find(|previous| previous.id == tab.id);
      if previous.is_none() {
        events.push(UiEvent {
          name: "tab_created".to_owned(),
          session_number: Some(new.number.clone()),
          workspace_id: Some(workspace.id),
          workspace_index: Some(workspace_index + 1),
          tab_id: Some(tab.id),
          index: Some(tab_index + 1),
          ..UiEvent::default()
        });
      }
      if let Some(previous) = previous {
        if previous.title != tab.title {
          events.push(UiEvent {
            name: "terminal_title_changed".to_owned(),
            session_number: Some(new.number.clone()),
            workspace_id: Some(workspace.id),
            tab_id: Some(tab.id),
            title: Some(tab.title.clone()),
            ..UiEvent::default()
          });
        }
        if previous.exit_code.is_none() && tab.exit_code.is_some() {
          events.push(UiEvent {
            name: "process_exited".to_owned(),
            session_number: Some(new.number.clone()),
            workspace_id: Some(workspace.id),
            workspace_index: Some(workspace_index + 1),
            tab_id: Some(tab.id),
            index: Some(tab_index + 1),
            exit_code: tab.exit_code,
            ..UiEvent::default()
          });
        }
      }
    }
  }
  for (workspace_index, workspace) in old.workspaces.iter().enumerate() {
    for (tab_index, tab) in workspace.tabs.iter().enumerate() {
      if !new
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.tabs.iter())
        .any(|current| current.id == tab.id)
      {
        events.push(UiEvent {
          name: "tab_closed".to_owned(),
          session_number: Some(new.number.clone()),
          workspace_id: Some(workspace.id),
          workspace_index: Some(workspace_index + 1),
          tab_id: Some(tab.id),
          index: Some(tab_index + 1),
          ..UiEvent::default()
        });
      }
    }
  }

  if old.active_workspace_id != new.active_workspace_id {
    let workspace = new
      .workspaces
      .iter()
      .find(|workspace| workspace.id == new.active_workspace_id);
    events.push(UiEvent {
      name: "workspace_selected".to_owned(),
      session_number: Some(new.number.clone()),
      workspace_id: Some(new.active_workspace_id),
      workspace_index: workspace.and_then(|workspace| {
        new
          .workspaces
          .iter()
          .position(|candidate| candidate.id == workspace.id)
          .map(|index| index + 1)
      }),
      previous_workspace_id: Some(old.active_workspace_id),
      ..UiEvent::default()
    });
  }
  let previous_tab = old
    .workspaces
    .iter()
    .find(|workspace| workspace.id == old.active_workspace_id)
    .map(|workspace| workspace.active_tab_id);
  let current_tab = new
    .workspaces
    .iter()
    .find(|workspace| workspace.id == new.active_workspace_id)
    .map(|workspace| workspace.active_tab_id);
  if previous_tab != current_tab {
    let workspace = new
      .workspaces
      .iter()
      .find(|workspace| workspace.id == new.active_workspace_id);
    let index = workspace.and_then(|workspace| {
      workspace
        .tabs
        .iter()
        .position(|tab| Some(tab.id) == current_tab)
        .map(|index| index + 1)
    });
    let previous_index = old
      .workspaces
      .iter()
      .find(|workspace| workspace.id == old.active_workspace_id)
      .and_then(|workspace| {
        workspace
          .tabs
          .iter()
          .position(|tab| Some(tab.id) == previous_tab)
          .map(|index| index + 1)
      });
    events.push(UiEvent {
      name: "tab_selected".to_owned(),
      session_number: Some(new.number.clone()),
      workspace_id: Some(new.active_workspace_id),
      tab_id: current_tab,
      index,
      previous_index,
      ..UiEvent::default()
    });
  }
  events
}

/// Routes one terminal event through modal, drawer, key-dispatch, and resize
/// handling.
///
/// Modal and drawer controls take precedence over normal key dispatch unless
/// the leader/prefix sequence is active. Normal keys are converted into
/// forwarded PTY bytes, Lua callback execution, warnings, or ignored input.
fn handle_input(
  event: Event,
  context: &mut ClientContext<'_>,
) -> Result<Effects, Box<dyn std::error::Error>> {
  if let Event::Key(key) = &event
    && key.kind == KeyEventKind::Release
  {
    if let Some((code, commands)) = context.session.pending_shutdown.take() {
      if code == key.code {
        context
          .trace
          .mark(&format!("shutdown-key-released {key:?}"));
        return execute_runtime_commands(commands, context);
      }
      context.session.pending_shutdown = Some((code, commands));
    }
    return Ok(Effects::default());
  }
  if context.session.pending_shutdown.is_some() {
    context
      .trace
      .mark(&format!("shutdown-suppressed-event {event:?}"));
    return Ok(Effects::default());
  }

  let leader_input = matches!(&event, Event::Key(key) if context.dispatcher.is_leader()
        || context.dispatcher.is_prefix_key(*key));
  if context.session.lua_picker.is_some()
    && !leader_input
    && let Event::Key(key) = &event
  {
    return handle_lua_picker_key(*key, context);
  }
  if context.session.workspace_modal.is_some()
    && !leader_input
    && let Event::Key(key) = &event
  {
    return handle_workspace_modal_key(*key, context);
  }
  if context.dispatcher.is_leader()
    && let Event::Key(key) = &event
    && matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
    && matches!(
      key.code,
      crossterm::event::KeyCode::Char('[') | crossterm::event::KeyCode::Char(']')
    )
  {
    let popup = WhichKeyPopup {
      prefix: context.dispatcher.sequence().to_vec(),
      completions: context.dispatcher.completions(),
      scroll_offset: context.session.which_key_scroll,
      options: context.session.which_key_options,
    };
    let viewport = Rect::new(0, 0, context.outer_size.0, context.outer_size.1);
    if popup.can_scroll(viewport) {
      let max_scroll = popup.max_scroll(viewport);
      context.session.which_key_scroll = match key.code {
        crossterm::event::KeyCode::Char('[') => context.session.which_key_scroll.saturating_sub(1),
        crossterm::event::KeyCode::Char(']') => context
          .session
          .which_key_scroll
          .saturating_add(1)
          .min(max_scroll),
        _ => context.session.which_key_scroll,
      };
      return Ok(Effects {
        redraw: true,
        ..Effects::default()
      });
    }
  }
  if context.session.drawer_expanded
    && !leader_input
    && let Event::Key(key) = &event
  {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
      return Ok(Effects::default());
    }
    let workspaces = &context.session.snapshot.workspaces;
    let active_id = context.session.snapshot.active_workspace_id;
    let current_id = context
      .session
      .drawer_highlighted_workspace
      .unwrap_or(active_id);
    let current_index = workspaces
      .iter()
      .position(|workspace| workspace.id == current_id)
      .unwrap_or_default();
    match key.code {
      crossterm::event::KeyCode::Esc => {
        context.session.drawer_expanded = false;
        return Ok(Effects {
          redraw: true,
          ..Effects::default()
        });
      }
      crossterm::event::KeyCode::Up if !workspaces.is_empty() => {
        let index = if current_index == 0 {
          workspaces.len() - 1
        } else {
          current_index - 1
        };
        context.session.drawer_highlighted_workspace = Some(workspaces[index].id);
        return Ok(Effects {
          redraw: true,
          ..Effects::default()
        });
      }
      crossterm::event::KeyCode::Down if !workspaces.is_empty() => {
        let index = (current_index + 1) % workspaces.len();
        context.session.drawer_highlighted_workspace = Some(workspaces[index].id);
        return Ok(Effects {
          redraw: true,
          ..Effects::default()
        });
      }
      crossterm::event::KeyCode::Enter => {
        if let Some(workspace_id) = context.session.drawer_highlighted_workspace
          && workspace_id != active_id
        {
          send_request(
            context.sender,
            context.next_request_id,
            ClientRequest::SelectWorkspace(workspace_id),
          )?;
        }
        context.session.drawer_expanded = false;
        context.session.drawer_highlighted_workspace = None;
        return Ok(Effects {
          redraw: true,
          ..Effects::default()
        });
      }
      _ => return Ok(Effects::default()),
    }
  }
  match event {
    Event::Key(key) if key.kind != KeyEventKind::Release => {
      context.trace.mark("client-key-dispatched");
      let now = Instant::now();
      let was_leader = context.dispatcher.is_leader();
      context.trace.mark(&format!("input-dispatch-event {key:?}"));
      let outcome = context.dispatcher.handle_event(key, now);
      let leader_changed = was_leader != context.dispatcher.is_leader();
      if leader_changed && !context.dispatcher.is_leader() {
        context.session.which_key_scroll = 0;
      }
      let mut effects = Effects::default();
      match outcome {
        KeyOutcome::PrefixEntered => {
          context.session.which_key_scroll = 0;
          effects.redraw = true;
          effects.events.push(UiEvent {
            name: "leader_mode_changed".to_owned(),
            active: Some(true),
            ..UiEvent::default()
          });
        }
        KeyOutcome::PrefixCancelled => {
          effects.redraw = true;
          effects.events.push(UiEvent {
            name: "leader_mode_changed".to_owned(),
            active: Some(false),
            ..UiEvent::default()
          });
        }
        KeyOutcome::Execute(callback) => {
          effects.redraw = true;
          match context.lua.invoke_key_callback(&callback) {
            Ok(commands) => {
              if commands.iter().any(|command| {
                matches!(command, RuntimeCommand::Detach | RuntimeCommand::StopServer)
              }) {
                context
                  .trace
                  .mark(&format!("shutdown-waiting-for-key-release {key:?}"));
                context.session.pending_shutdown = Some((key.code, commands));
              } else {
                effects.merge(execute_runtime_commands(commands, context)?);
              }
            }
            Err(error) => {
              context.session.key_warning =
                Some((format!("Key callback: {error}"), now + Duration::from_secs(3)));
              effects.redraw = true;
            }
          }
          effects.redraw |= leader_changed;
          if leader_changed {
            effects.events.push(UiEvent {
              name: "leader_mode_changed".to_owned(),
              active: Some(false),
              ..UiEvent::default()
            });
          }
        }
        KeyOutcome::Forward(bytes) => {
          context
            .trace
            .mark(&format!("input-forwarded bytes={bytes:?}"));
          send_request(context.sender, context.next_request_id, ClientRequest::Input(bytes))?
        }
        KeyOutcome::Unbound(key) => {
          context.session.key_warning =
            Some((format!("Unbound key: {key}"), now + Duration::from_secs(3)));
          effects.redraw = true;
          if leader_changed {
            effects.events.push(UiEvent {
              name: "leader_mode_changed".to_owned(),
              active: Some(false),
              ..UiEvent::default()
            });
          }
        }
        KeyOutcome::Ignored => effects.redraw |= leader_changed || context.dispatcher.is_leader(),
      }
      Ok(effects)
    }
    Event::Resize(width, height) => {
      *context.outer_size = (width, height);
      let options = context.lua.sync_options()?;
      let pane = ui::pane_size(width, height, options, context.session.drawer_expanded);
      let viewport = Viewport {
        rows: pane.0.max(1),
        columns: pane.1.max(1),
      };
      if viewport != *context.viewport {
        *context.viewport = viewport;
        context.session.resize(viewport);
        send_request(context.sender, context.next_request_id, ClientRequest::Resize(viewport))?;
      }
      Ok(Effects {
        redraw: true,
        events: vec![UiEvent {
          name: "terminal_resized".to_owned(),
          rows: Some(height),
          columns: Some(width),
          ..UiEvent::default()
        }],
        ..Effects::default()
      })
    }
    _ => Ok(Effects::default()),
  }
}

/// Handles navigation, filtering, cancellation, and selection in a Lua picker.
fn handle_lua_picker_key(
  key: crossterm::event::KeyEvent,
  context: &mut ClientContext<'_>,
) -> Result<Effects, Box<dyn std::error::Error>> {
  if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
    return Ok(Effects::default());
  }
  let Some(mut picker) = context.session.lua_picker.take() else {
    return Ok(Effects::default());
  };
  match key.code {
    crossterm::event::KeyCode::Esc => {
      let commands = context.lua.complete_picker(picker.id, None)?;
      let effects = execute_runtime_commands(commands, context)?;
      Ok(Effects {
        redraw: true,
        ..effects
      })
    }
    crossterm::event::KeyCode::Enter => {
      let visible = zmux::picker::fuzzy_filter(&picker.query, &picker.labels);
      let Some(selected_index) = visible.get(picker.highlighted).copied() else {
        context.session.lua_picker = Some(picker);
        return Ok(Effects::default());
      };
      let commands = context
        .lua
        .complete_picker(picker.id, Some(selected_index))?;
      let effects = execute_runtime_commands(commands, context)?;
      Ok(Effects {
        redraw: true,
        ..effects
      })
    }
    crossterm::event::KeyCode::Up => {
      let count = zmux::picker::fuzzy_filter(&picker.query, &picker.labels).len();
      if count > 0 {
        picker.highlighted = if picker.highlighted == 0 {
          count - 1
        } else {
          picker.highlighted - 1
        };
      }
      context.session.lua_picker = Some(picker);
      Ok(Effects {
        redraw: true,
        ..Effects::default()
      })
    }
    crossterm::event::KeyCode::Down => {
      let count = zmux::picker::fuzzy_filter(&picker.query, &picker.labels).len();
      if count > 0 {
        picker.highlighted = (picker.highlighted + 1) % count;
      }
      context.session.lua_picker = Some(picker);
      Ok(Effects {
        redraw: true,
        ..Effects::default()
      })
    }
    crossterm::event::KeyCode::Backspace => {
      picker.query.pop();
      picker.highlighted = 0;
      context.session.lua_picker = Some(picker);
      Ok(Effects {
        redraw: true,
        ..Effects::default()
      })
    }
    crossterm::event::KeyCode::Char(character)
      if !key.modifiers.intersects(
        crossterm::event::KeyModifiers::CONTROL | crossterm::event::KeyModifiers::ALT,
      ) =>
    {
      picker.query.push(character);
      picker.highlighted = 0;
      context.session.lua_picker = Some(picker);
      Ok(Effects {
        redraw: true,
        ..Effects::default()
      })
    }
    _ => {
      context.session.lua_picker = Some(picker);
      Ok(Effects::default())
    }
  }
}

/// Handles keyboard input for workspace forms and directory selection.
///
/// The form delegates directory browsing to [`DirectoryPicker`], while Enter
/// always submits the current create/edit values.
fn handle_workspace_modal_key(
  key: crossterm::event::KeyEvent,
  context: &mut ClientContext<'_>,
) -> Result<Effects, Box<dyn std::error::Error>> {
  if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
    return Ok(Effects::default());
  }
  let Some(modal) = context.session.workspace_modal.take() else {
    return Ok(Effects::default());
  };
  match modal {
    WorkspaceModal::Form(mut form) => match key.code {
      crossterm::event::KeyCode::Esc => Ok(Effects {
        redraw: true,
        ..Effects::default()
      }),
      crossterm::event::KeyCode::Tab => {
        let backwards = key
          .modifiers
          .contains(crossterm::event::KeyModifiers::SHIFT);
        form.field = match (form.field, backwards) {
          (WorkspaceField::Directory, false) | (WorkspaceField::Name, true) => WorkspaceField::Name,
          (WorkspaceField::Name, false) | (WorkspaceField::Directory, true) => {
            WorkspaceField::Directory
          }
        };
        form.editing = false;
        context.session.workspace_modal = Some(WorkspaceModal::Form(form));
        Ok(Effects {
          redraw: true,
          ..Effects::default()
        })
      }
      crossterm::event::KeyCode::Char('e')
        if !key.modifiers.intersects(
          crossterm::event::KeyModifiers::CONTROL | crossterm::event::KeyModifiers::ALT,
        ) =>
      {
        match form.field {
          WorkspaceField::Directory => match DirectoryPicker::new(form) {
            Ok(picker) => {
              context.session.workspace_modal = Some(WorkspaceModal::Directory(picker));
            }
            Err(error) => {
              context.session.last_error = Some(error.to_string());
              context.session.workspace_modal = None;
            }
          },
          WorkspaceField::Name => {
            form.editing = true;
            context.session.workspace_modal = Some(WorkspaceModal::Form(form));
          }
        }
        Ok(Effects {
          redraw: true,
          ..Effects::default()
        })
      }
      crossterm::event::KeyCode::Enter => {
        let name = (!form.name.trim().is_empty()).then_some(form.name);
        let request = match form.mode {
          WorkspaceDialogMode::Create => ClientRequest::CreateWorkspace {
            working_directory: form.working_directory,
            name,
          },
          WorkspaceDialogMode::Edit(id) => ClientRequest::EditWorkspace {
            id,
            working_directory: form.working_directory,
            name,
          },
        };
        send_request(context.sender, context.next_request_id, request)?;
        context.session.workspace_modal = None;
        context.session.last_error = None;
        Ok(Effects {
          redraw: true,
          ..Effects::default()
        })
      }
      crossterm::event::KeyCode::Backspace
        if form.field == WorkspaceField::Name && form.editing =>
      {
        form.name.pop();
        context.session.workspace_modal = Some(WorkspaceModal::Form(form));
        Ok(Effects {
          redraw: true,
          ..Effects::default()
        })
      }
      crossterm::event::KeyCode::Char(character)
        if form.field == WorkspaceField::Name
          && form.editing
          && !key.modifiers.intersects(
            crossterm::event::KeyModifiers::CONTROL | crossterm::event::KeyModifiers::ALT,
          ) =>
      {
        form.name.push(character);
        context.session.workspace_modal = Some(WorkspaceModal::Form(form));
        Ok(Effects {
          redraw: true,
          ..Effects::default()
        })
      }
      _ => {
        context.session.workspace_modal = Some(WorkspaceModal::Form(form));
        Ok(Effects::default())
      }
    },
    WorkspaceModal::Directory(mut picker) => match key.code {
      crossterm::event::KeyCode::Esc => {
        context.session.workspace_modal = Some(WorkspaceModal::Form(picker.form));
        Ok(Effects {
          redraw: true,
          ..Effects::default()
        })
      }
      crossterm::event::KeyCode::Up => {
        picker.move_highlight(-1);
        context.session.workspace_modal = Some(WorkspaceModal::Directory(picker));
        Ok(Effects {
          redraw: true,
          ..Effects::default()
        })
      }
      crossterm::event::KeyCode::Down => {
        picker.move_highlight(1);
        context.session.workspace_modal = Some(WorkspaceModal::Directory(picker));
        Ok(Effects {
          redraw: true,
          ..Effects::default()
        })
      }
      crossterm::event::KeyCode::Enter => match picker.confirm() {
        Ok(Some(directory)) => {
          let mut form = picker.form;
          form.working_directory = directory;
          form.editing = false;
          context.session.workspace_modal = Some(WorkspaceModal::Form(form));
          Ok(Effects {
            redraw: true,
            ..Effects::default()
          })
        }
        Ok(None) => {
          context.session.workspace_modal = Some(WorkspaceModal::Directory(picker));
          Ok(Effects {
            redraw: true,
            ..Effects::default()
          })
        }
        Err(error) => {
          context.session.last_error = Some(error.to_string());
          context.session.workspace_modal = Some(WorkspaceModal::Directory(picker));
          Ok(Effects {
            redraw: true,
            ..Effects::default()
          })
        }
      },
      crossterm::event::KeyCode::Backspace => {
        picker.query.pop();
        picker.highlighted = 0;
        context.session.workspace_modal = Some(WorkspaceModal::Directory(picker));
        Ok(Effects {
          redraw: true,
          ..Effects::default()
        })
      }
      crossterm::event::KeyCode::Char(character)
        if !key.modifiers.intersects(
          crossterm::event::KeyModifiers::CONTROL | crossterm::event::KeyModifiers::ALT,
        ) =>
      {
        picker.query.push(character);
        picker.highlighted = 0;
        context.session.workspace_modal = Some(WorkspaceModal::Directory(picker));
        Ok(Effects {
          redraw: true,
          ..Effects::default()
        })
      }
      _ => {
        context.session.workspace_modal = Some(WorkspaceModal::Directory(picker));
        Ok(Effects::default())
      }
    },
  }
}

/// Assigns a monotonically increasing request ID and queues a server request.
fn send_request(
  sender: &Sender<ClientMessage>,
  next_id: &mut u64,
  request: ClientRequest,
) -> Result<(), Box<dyn std::error::Error>> {
  let id = *next_id;
  *next_id = next_id.saturating_add(1);
  sender.send(ClientMessage::Request { id, request })?;
  Ok(())
}

/// Delivers queued application events to Lua handlers.
///
/// Handlers may emit more events or runtime commands. The safety limit prevents
/// recursive callbacks from monopolizing the client forever; viewport changes
/// caused by Lua options are sent to the server before command execution
/// continues.
fn dispatch_ui_events(
  effects: Effects,
  context: &mut ClientContext<'_>,
) -> Result<Effects, Box<dyn std::error::Error>> {
  let mut effects = effects;
  let mut pending: VecDeque<_> = effects.events.drain(..).collect();
  let mut dispatched = 0;
  while let Some(event) = pending.pop_front() {
    dispatched += 1;
    if dispatched > MAX_CALLBACK_COMMANDS {
      return Err("Lua callback command/event loop exceeded its safety limit".into());
    }
    for callback in context.lua.event_handlers(&event.name) {
      context.lua.sync_state(&context.session.snapshot_for_lua(
        context.dispatcher.is_leader(),
        context.outer_size.0,
        context.outer_size.1,
      ))?;
      let commands = context.lua.invoke_event_handler(&callback, &event)?;
      let options = context.lua.sync_options()?;
      let pane = ui::pane_size(
        context.outer_size.0,
        context.outer_size.1,
        options,
        context.session.drawer_expanded,
      );
      let next_viewport = Viewport {
        rows: pane.0.max(1),
        columns: pane.1.max(1),
      };
      if next_viewport != *context.viewport {
        *context.viewport = next_viewport;
        context.session.resize(next_viewport);
        send_request(
          context.sender,
          context.next_request_id,
          ClientRequest::Resize(next_viewport),
        )?;
        effects.redraw = true;
      }
      effects.merge(execute_runtime_commands(commands, context)?);
    }
  }
  Ok(effects)
}

/// Applies commands emitted by Lua callbacks.
///
/// Local commands update picker/drawer/modal state. Session and tab commands
/// become requests for the server, preserving the server as the authority for
/// session structure.
fn execute_runtime_commands(
  commands: Vec<RuntimeCommand>,
  context: &mut ClientContext<'_>,
) -> Result<Effects, Box<dyn std::error::Error>> {
  let mut effects = Effects::default();
  for command in commands {
    match command {
      RuntimeCommand::Redraw => {
        context.trace.mark("ui-redraw-requested");
        effects.redraw = true;
      }
      RuntimeCommand::OpenPicker(id) => {
        let Some(labels) = context.lua.picker_labels(id) else {
          context.session.last_error = Some("picker request expired".to_owned());
          effects.redraw = true;
          continue;
        };
        context.session.lua_picker = Some(LuaPickerState {
          id,
          labels,
          query: String::new(),
          highlighted: 0,
        });
        effects.redraw = true;
      }
      RuntimeCommand::ToggleWorkspaceDrawer => {
        context.session.drawer_expanded = !context.session.drawer_expanded;
        context.session.drawer_highlighted_workspace = context
          .session
          .drawer_expanded
          .then_some(context.session.snapshot.active_workspace_id);
        effects.redraw = true;
      }
      RuntimeCommand::CreateWorkspace => {
        context.session.open_create_workspace()?;
        effects.redraw = true;
      }
      RuntimeCommand::EditWorkspace => {
        context.session.open_edit_workspace()?;
        effects.redraw = true;
      }
      command @ (RuntimeCommand::NextWorkspace | RuntimeCommand::PreviousWorkspace) => {
        let workspaces = &context.session.snapshot.workspaces;
        if workspaces.len() > 1 {
          let current = workspaces
            .iter()
            .position(|workspace| workspace.id == context.session.snapshot.active_workspace_id)
            .unwrap_or_default();
          let next = if matches!(command, RuntimeCommand::NextWorkspace) {
            (current + 1) % workspaces.len()
          } else {
            (current + workspaces.len() - 1) % workspaces.len()
          };
          send_request(
            context.sender,
            context.next_request_id,
            ClientRequest::SelectWorkspace(workspaces[next].id),
          )?;
        }
      }
      RuntimeCommand::SelectWorkspace(id) => {
        send_request(context.sender, context.next_request_id, ClientRequest::SelectWorkspace(id))?;
      }
      RuntimeCommand::CloseWorkspace => {
        send_request(context.sender, context.next_request_id, ClientRequest::CloseWorkspace)?;
      }
      RuntimeCommand::Detach => {
        send_request(context.sender, context.next_request_id, ClientRequest::Detach)?;
        effects.quit = true;
      }
      RuntimeCommand::KillSession => {
        send_request(context.sender, context.next_request_id, ClientRequest::KillSession)?
      }
      RuntimeCommand::StopServer => {
        send_request(context.sender, context.next_request_id, ClientRequest::StopServer)?;
        effects.quit = true;
      }
      RuntimeCommand::NewTab => {
        send_request(context.sender, context.next_request_id, ClientRequest::NewTab)?
      }
      RuntimeCommand::CloseTab => {
        send_request(context.sender, context.next_request_id, ClientRequest::CloseTab)?
      }
      RuntimeCommand::NextTab => {
        send_request(context.sender, context.next_request_id, ClientRequest::NextTab)?
      }
      RuntimeCommand::PreviousTab => {
        send_request(context.sender, context.next_request_id, ClientRequest::PreviousTab)?
      }
      RuntimeCommand::SelectTab(index) => {
        send_request(context.sender, context.next_request_id, ClientRequest::SelectTab(index))?
      }
    }
  }
  Ok(effects)
}

/// Evaluates the six configured Lua bar parts for the next frame.
///
/// Missing parts remain `None`, allowing [`ui::draw`] to supply its built-in
/// defaults; an explicitly empty Lua result suppresses that group.
fn render_bars(lua: &LuaConfig) -> mlua::Result<ui::BarContents> {
  Ok(ui::BarContents {
    top: ui::BarParts {
      left: lua.render_bar_part("top", "left")?,
      center: lua.render_bar_part("top", "center")?,
      right: lua.render_bar_part("top", "right")?,
    },
    bottom: ui::BarParts {
      left: lua.render_bar_part("bottom", "left")?,
      center: lua.render_bar_part("bottom", "center")?,
      right: lua.render_bar_part("bottom", "right")?,
    },
  })
}

#[cfg(test)]
mod tests {
  use zmux::protocol::TerminalSnapshot;

  use super::{
    DirectoryPicker,
    TerminalView,
    WorkspaceDialog,
    WorkspaceDialogMode,
    WorkspaceField,
  };

  #[test]
  fn terminal_parser_uses_shared_pty_geometry_not_client_viewport() {
    let view = TerminalView::from_snapshot(TerminalSnapshot {
      tab_id: 1,
      rows: 40,
      columns: 120,
      bytes: Vec::new(),
      title: "shell".to_owned(),
      exit_code: None,
    });

    assert_eq!(view.emulator.size(), (40, 120));
  }

  #[test]
  fn empty_terminal_titles_use_a_stable_fallback() {
    let view = TerminalView::from_snapshot(TerminalSnapshot {
      tab_id: 7,
      rows: 2,
      columns: 4,
      bytes: Vec::new(),
      title: String::new(),
      exit_code: None,
    });

    assert_eq!(view.title, "Terminal 7");
  }

  #[test]
  fn directory_selector_fuzzily_finds_and_chooses_child_directories() {
    let root = std::env::temp_dir().join(format!("zmux-picker-test-{}", std::process::id()));
    let child = root.join("beta");
    std::fs::create_dir_all(&child).expect("create picker test directory");
    let form = WorkspaceDialog {
      mode: WorkspaceDialogMode::Create,
      working_directory: root.clone(),
      name: String::new(),
      field: WorkspaceField::Directory,
      editing: false,
    };
    let mut picker = DirectoryPicker::new(form).expect("open directory picker");
    picker.query = "bt".to_owned();
    picker.highlighted = 0;

    assert_eq!(picker.visible_indices().len(), 1);
    assert!(picker.confirm().expect("navigate into child").is_none());
    assert_eq!(picker.directory, child);
    assert_eq!(picker.confirm().expect("choose current directory"), Some(child));

    std::fs::remove_dir_all(root).expect("remove picker test directory");
  }
}
