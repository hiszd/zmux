mod headless_commands;

use std::{
  collections::{
    HashMap,
    HashSet,
  },
  io,
  path::PathBuf,
  sync::{
    Arc,
    atomic::{
      AtomicBool,
      Ordering,
    },
    mpsc::{
      self,
      Receiver,
      Sender,
    },
  },
  thread,
  time::{
    Duration,
    Instant,
  },
};

use interprocess::local_socket::prelude::*;
use zmux::{
  domain::{
    Session,
    SessionName,
    SessionTabCloseOutcome,
    TabId,
    Workspace,
    WorkspaceId,
  },
  protocol::{
    AdminRequest,
    AdminResponse,
    ClientHello,
    ClientMessage,
    ClientRequest,
    HeadlessHello,
    HeadlessRequest,
    ServerMessage,
    SessionSnapshot,
    SessionSummary,
    TabSnapshot as WireTabSnapshot,
    TerminalSnapshot,
    Viewport,
    WorkspaceSnapshot,
    read_frame,
    write_frame,
  },
  transport::{
    self,
    Endpoint,
  },
};

use crate::{
  events::AppEvent,
  pane::Pane,
  trace::Trace,
};

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

struct ServerTab {
  id: TabId,
  name: String,
  pane: Pane,
  title: String,
  explicit_title: bool,
  exit_code: Option<u32>,
}

#[derive(Clone)]
struct ShellConfig {
  executable: std::path::PathBuf,
  environment: Vec<(String, String)>,
}

struct ServerSession {
  model: Session,
  tabs: HashMap<TabId, ServerTab>,
  clients: HashSet<u64>,
  last_pty_size: Viewport,
  shell: ShellConfig,
}

struct ClientConnection {
  session_name: String,
  viewport: Viewport,
  sender: Sender<ServerMessage>,
}

struct HeadlessConnection {
  working_directory: PathBuf,
  shell: ShellConfig,
  sender: Sender<ServerMessage>,
}

enum ServerEvent {
  Connected {
    client_id: u64,
    hello: ClientHello,
    sender: Sender<ServerMessage>,
  },
  Admin {
    request: AdminRequest,
    sender: Sender<ServerMessage>,
  },
  HeadlessConnected {
    connection_id: u64,
    hello: HeadlessHello,
    sender: Sender<ServerMessage>,
  },
  HeadlessRequest {
    connection_id: u64,
    id: u64,
    request: HeadlessRequest,
  },
  Request {
    client_id: u64,
    id: u64,
    request: ClientRequest,
  },
  Disconnected(u64),
  PtyOutput {
    tab_id: u64,
    bytes: Vec<u8>,
  },
}

pub fn run(endpoint: Endpoint) -> Result<(), Box<dyn std::error::Error>> {
  let listener = transport::bind(&endpoint)?;
  let (events, receiver) = mpsc::channel();
  let (pty_events, pty_receiver) = mpsc::channel();
  forward_pty_events(pty_receiver, events.clone());
  let stop_accepting = Arc::new(AtomicBool::new(false));
  let accept_thread = accept_connections(listener, events.clone(), stop_accepting.clone())?;

  let mut server = ServerState::new(pty_events, Trace::from_environment());
  loop {
    match receiver.recv_timeout(PROCESS_POLL_INTERVAL) {
      Ok(event) => server.handle_event(event)?,
      Err(mpsc::RecvTimeoutError::Timeout) => server.poll_processes()?,
      Err(mpsc::RecvTimeoutError::Disconnected) => break,
    }
    if server.should_stop {
      break;
    }
  }
  stop_accepting.store(true, Ordering::SeqCst);
  let _ = transport::connect(&endpoint);
  let _ = accept_thread.join();
  Ok(())
}

fn accept_connections(
  listener: LocalSocketListener,
  events: Sender<ServerEvent>,
  stop: Arc<AtomicBool>,
) -> io::Result<thread::JoinHandle<()>> {
  thread::Builder::new()
    .name("zmux-ipc-accept".to_owned())
    .spawn(move || {
      let mut next_client_id = 1_u64;
      while !stop.load(Ordering::SeqCst) {
        match transport::accept(&listener) {
          Ok(_stream) if stop.load(Ordering::SeqCst) => break,
          Ok(stream) => {
            let client_id = next_client_id;
            next_client_id = next_client_id.saturating_add(1);
            if spawn_connection(stream, client_id, events.clone()).is_err() {
              continue;
            }
          }
          Err(_) if stop.load(Ordering::SeqCst) => break,
          Err(error) => eprintln!("zmux server: failed to accept client: {error}"),
        }
      }
    })
}

fn spawn_connection(
  stream: LocalSocketStream,
  client_id: u64,
  events: Sender<ServerEvent>,
) -> io::Result<()> {
  let (mut reader, mut writer) = stream.split();
  let (to_client, outgoing) = mpsc::channel::<ServerMessage>();
  thread::Builder::new()
    .name(format!("zmux-ipc-writer-{client_id}"))
    .spawn(move || {
      while let Ok(message) = outgoing.recv() {
        if write_frame(&mut writer, &message).is_err() {
          break;
        }
      }
    })?;

  thread::Builder::new()
    .name(format!("zmux-ipc-reader-{client_id}"))
    .spawn(move || {
      let first_message = match read_frame::<_, ClientMessage>(&mut reader) {
        Ok(message) => message,
        Err(_) => return,
      };
      let hello = match first_message {
        ClientMessage::Hello(hello) => hello,
        ClientMessage::Admin(request) => {
          let _ = events.send(ServerEvent::Admin {
            request,
            sender: to_client,
          });
          return;
        }
        ClientMessage::HeadlessHello(hello) => {
          if hello.protocol_version != zmux::protocol::PROTOCOL_VERSION {
            let _ = to_client.send(ServerMessage::HelloRejected {
              reason: format!(
                "protocol version mismatch: client {}, server {}",
                hello.protocol_version,
                zmux::protocol::PROTOCOL_VERSION
              ),
            });
            return;
          }
          let _ = events.send(ServerEvent::HeadlessConnected {
            connection_id: client_id,
            hello,
            sender: to_client.clone(),
          });
          while let Ok(ClientMessage::HeadlessRequest { id, request }) =
            read_frame::<_, ClientMessage>(&mut reader)
          {
            if events
              .send(ServerEvent::HeadlessRequest {
                connection_id: client_id,
                id,
                request,
              })
              .is_err()
            {
              break;
            }
          }
          let _ = events.send(ServerEvent::Disconnected(client_id));
          return;
        }
        ClientMessage::Request { .. } | ClientMessage::HeadlessRequest { .. } => return,
      };
      if events
        .send(ServerEvent::Connected {
          client_id,
          hello,
          sender: to_client.clone(),
        })
        .is_err()
      {
        return;
      }
      while let Ok(ClientMessage::Request { id, request }) =
        read_frame::<_, ClientMessage>(&mut reader)
      {
        if events
          .send(ServerEvent::Request {
            client_id,
            id,
            request,
          })
          .is_err()
        {
          break;
        }
      }
      let _ = events.send(ServerEvent::Disconnected(client_id));
    })?;
  Ok(())
}

fn forward_pty_events(receiver: Receiver<AppEvent>, events: Sender<ServerEvent>) {
  thread::Builder::new()
    .name("zmux-pty-events".to_owned())
    .spawn(move || {
      while let Ok(AppEvent::PtyOutput { tab_id, bytes }) = receiver.recv() {
        if events
          .send(ServerEvent::PtyOutput { tab_id, bytes })
          .is_err()
        {
          break;
        }
      }
    })
    .expect("could not start PTY event forwarder");
}

struct ServerState {
  sessions: Vec<ServerSession>,
  clients: HashMap<u64, ClientConnection>,
  headless: HashMap<u64, HeadlessConnection>,
  next_workspace_id: u64,
  next_tab_id: u64,
  pty_events: Sender<AppEvent>,
  trace: Trace,
  should_stop: bool,
  has_received_client: bool,
}

impl ServerState {
  fn new(pty_events: Sender<AppEvent>, trace: Trace) -> Self {
    Self {
      sessions: Vec::new(),
      clients: HashMap::new(),
      headless: HashMap::new(),
      next_workspace_id: 1,
      next_tab_id: 1,
      pty_events,
      trace,
      should_stop: false,
      has_received_client: false,
    }
  }

  fn handle_event(&mut self, event: ServerEvent) -> Result<(), Box<dyn std::error::Error>> {
    match event {
      ServerEvent::Connected {
        client_id,
        hello,
        sender,
      } => self.connect_client(client_id, hello, sender)?,
      ServerEvent::Admin { request, sender } => self.handle_admin(request, sender)?,
      ServerEvent::HeadlessConnected {
        connection_id,
        hello,
        sender,
      } => {
        self.headless.insert(connection_id, HeadlessConnection {
          working_directory: hello.working_directory,
          shell: ShellConfig {
            executable: hello.shell,
            environment: hello.environment,
          },
          sender,
        });
      }
      ServerEvent::HeadlessRequest {
        connection_id,
        id,
        request,
      } => {
        self.handle_headless_request(connection_id, id, request)?;
      }
      ServerEvent::Request {
        client_id,
        id,
        request,
      } => self.handle_request(client_id, id, request)?,
      ServerEvent::Disconnected(client_id) => {
        self.headless.remove(&client_id);
        self.disconnect_client(client_id)?;
      }
      ServerEvent::PtyOutput { tab_id, bytes } => self.handle_pty_output(tab_id, bytes),
    }
    if self.has_received_client && self.sessions.is_empty() {
      self.should_stop = true;
    }
    Ok(())
  }

  fn handle_admin(
    &mut self,
    request: AdminRequest,
    sender: Sender<ServerMessage>,
  ) -> Result<(), Box<dyn std::error::Error>> {
    let result = match request {
      AdminRequest::ListSessions => {
        let sessions = self
          .sessions
          .iter()
          .map(|session| SessionSummary {
            number: session.model.name().get().to_owned(),
            clients: session.clients.len(),
            workspaces: session.model.workspaces().len(),
            tabs: session.tabs.len(),
          })
          .collect();
        Ok(AdminResponse::Sessions(sessions))
      }
      AdminRequest::Status => Ok(AdminResponse::Status {
        running: true,
        sessions: self.sessions.len(),
      }),
      AdminRequest::KillSession(name) => self
        .close_session(&name)
        .map(|()| AdminResponse::Success)
        .map_err(|error| error.to_string()),
      AdminRequest::PrepareSession {
        session_name,
        create_if_missing,
        fail_if_exists,
        working_directory,
        shell,
        environment,
        viewport,
      } => {
        let existing = session_name.as_deref().and_then(|name| {
          self
            .sessions
            .iter()
            .find(|session| session.model.name().get() == name)
            .map(|session| session.model.name().get().to_owned())
        });
        if let Some(name) = existing {
          if fail_if_exists {
            Err(format!("Session {name} already exists"))
          } else {
            Ok(AdminResponse::SessionReady {
              name,
              created: false,
            })
          }
        } else if !create_if_missing {
          Err(session_name.map_or_else(
            || "a Session name is required".to_owned(),
            |name| format!("Session {name} does not exist"),
          ))
        } else {
          self
            .create_session(
              session_name.unwrap_or_default(),
              &working_directory,
              &shell,
              &environment,
              viewport,
            )
            .map(|name| AdminResponse::SessionReady {
              name,
              created: true,
            })
            .map_err(|error| error.to_string())
        }
      }
      AdminRequest::StopServer => {
        self.stop_server();
        Ok(AdminResponse::Success)
      }
    };
    sender.send(ServerMessage::AdminResult {
      result: result.map_err(|error: String| error),
    })?;
    Ok(())
  }

  fn connect_client(
    &mut self,
    client_id: u64,
    hello: ClientHello,
    sender: Sender<ServerMessage>,
  ) -> Result<(), Box<dyn std::error::Error>> {
    self.has_received_client = true;
    if hello.protocol_version != zmux::protocol::PROTOCOL_VERSION {
      let _ = sender.send(ServerMessage::HelloRejected {
        reason: format!(
          "protocol version mismatch: client {}, server {}",
          hello.protocol_version,
          zmux::protocol::PROTOCOL_VERSION
        ),
      });
      return Ok(());
    }

    let session_name = if let Some(name) = hello.session_name {
      if let Some(session_name) = self
        .sessions
        .iter()
        .find(|session| session.model.name().get() == name)
        .map(|session| session.model.name().get().to_owned())
      {
        if hello.fail_if_exists {
          let _ = sender.send(ServerMessage::HelloRejected {
            reason: format!("Session {name} already exists"),
          });
          return Ok(());
        }
        session_name
      } else if hello.create_if_missing {
        match self.create_session(
          name,
          &hello.working_directory,
          &hello.shell,
          &hello.environment,
          hello.viewport,
        ) {
          Ok(session_name) => session_name,
          Err(error) => {
            let _ = sender.send(ServerMessage::HelloRejected {
              reason: error.to_string(),
            });
            return Ok(());
          }
        }
      } else {
        let _ = sender.send(ServerMessage::HelloRejected {
          reason: format!("Session {name} does not exist"),
        });
        return Ok(());
      }
    } else {
      match self.create_session(
        String::new(),
        &hello.working_directory,
        &hello.shell,
        &hello.environment,
        hello.viewport,
      ) {
        Ok(name) => name,
        Err(error) => {
          let _ = sender.send(ServerMessage::HelloRejected {
            reason: error.to_string(),
          });
          return Ok(());
        }
      }
    };

    self.clients.insert(client_id, ClientConnection {
      session_name: session_name.clone(),
      viewport: sanitize_viewport(hello.viewport),
      sender: sender.clone(),
    });
    if let Some(session) = self.session_mut(&session_name) {
      session.clients.insert(client_id);
    }
    let previous_size = self
      .session(&session_name)
      .map(|session| session.last_pty_size);
    self.resize_session(&session_name)?;
    let pty_size_changed = self
      .session(&session_name)
      .is_some_and(|session| Some(session.last_pty_size) != previous_size);

    sender.send(ServerMessage::HelloAccepted {
      client_id,
      session_name: session_name.clone(),
    })?;
    let Some(session) = self.session(&session_name) else {
      return Ok(());
    };
    sender.send(ServerMessage::InitialState {
      session: session.snapshot(),
      terminals: session.terminal_snapshots(),
    })?;
    if pty_size_changed {
      let _ = self.broadcast_state(&session_name);
    }
    Ok(())
  }

  fn create_session(
    &mut self,
    name: String,
    working_directory: &std::path::Path,
    shell: &std::path::Path,
    environment: &[(String, String)],
    size: Viewport,
  ) -> Result<String, Box<dyn std::error::Error>> {
    if name.contains(':') {
      return Err("names must not contain ':'".into());
    }
    if !working_directory.is_dir() {
      return Err(
        format!("working directory does not exist: {}", working_directory.display()).into(),
      );
    }
    let workspace_id = WorkspaceId::new(self.allocate_workspace_id()?);
    let tab_id = TabId::new(self.allocate_tab_id()?);
    let pty_size = sanitize_viewport(size);
    let pane = Pane::shell(
      pty_size.rows,
      pty_size.columns,
      tab_id.get(),
      working_directory,
      shell,
      environment,
      self.pty_events.clone(),
      self.trace.clone(),
    )?;
    let mut workspace = Workspace::new(workspace_id, working_directory.to_path_buf(), None);
    workspace.add_tab(tab_id);
    let name = if name.is_empty() {
      self.next_session_name()
    } else {
      name
    };
    if self
      .sessions
      .iter()
      .any(|session| session.model.name().get() == name)
    {
      return Err(format!("Session {name} already exists").into());
    }
    let model = Session::new(SessionName::new(name.clone()), workspace)?;
    let title = pane
      .foreground_process_name()
      .unwrap_or_else(|| format!("Terminal {}", tab_id.get()));
    let mut tabs = HashMap::new();
    tabs.insert(tab_id, ServerTab {
      id: tab_id,
      name: "1".to_owned(),
      pane,
      title,
      explicit_title: false,
      exit_code: None,
    });
    self.sessions.push(ServerSession {
      model,
      tabs,
      clients: HashSet::new(),
      last_pty_size: pty_size,
      shell: ShellConfig {
        executable: shell.to_owned(),
        environment: environment.to_owned(),
      },
    });
    Ok(name)
  }

  fn handle_request(
    &mut self,
    client_id: u64,
    request_id: u64,
    request: ClientRequest,
  ) -> Result<(), Box<dyn std::error::Error>> {
    let Some(connection) = self.clients.get(&client_id) else {
      return Ok(());
    };
    let session_name = connection.session_name.clone();
    let request_label = match &request {
      ClientRequest::SelectTab(_) => "select-tab",
      _ => "other",
    };
    let request_started = Instant::now();
    self.trace.mark(&format!(
      "server-request-start client_id={client_id} request_id={request_id} type={request_label}"
    ));
    let mut detach = false;
    let result = match request {
      ClientRequest::Input(bytes) => self.send_input(&session_name, &bytes),
      ClientRequest::ListSessions => {
        let sessions = self.session_summaries();
        if let Some(connection) = self.clients.get(&client_id) {
          connection
            .sender
            .send(ServerMessage::SessionList { sessions })
            .map_err(|error| error.to_string())?;
        }
        Ok(())
      }
      ClientRequest::SwitchSession(name) => self.switch_client_session(client_id, &name),
      ClientRequest::Resize(viewport) => {
        if let Some(connection) = self.clients.get_mut(&client_id) {
          connection.viewport = sanitize_viewport(viewport);
        }
        self
          .resize_session(&session_name)
          .map_err(|error| error.to_string())
          .and_then(|()| self.broadcast_state(&session_name))
      }
      ClientRequest::NewTab => self.create_tab(&session_name),
      ClientRequest::CloseTab => self.close_active_tab(&session_name),
      ClientRequest::SelectTab(index) => self.select_tab(&session_name, index),
      ClientRequest::CreateWorkspace {
        working_directory,
        name,
      } => self.create_workspace(&session_name, working_directory, name),
      ClientRequest::EditWorkspace {
        id,
        working_directory,
        name,
      } => self.edit_workspace(&session_name, id, working_directory, name),
      ClientRequest::SelectWorkspace(id) => self.select_workspace(&session_name, id),
      ClientRequest::CloseWorkspace => self.close_active_workspace(&session_name),
      ClientRequest::Detach => {
        detach = true;
        Ok(())
      }
      ClientRequest::KillSession => {
        self.close_session(&session_name)?;
        Ok(())
      }
      ClientRequest::StopServer => {
        self.stop_server();
        Ok(())
      }
    };

    if let Some(connection) = self.clients.get(&client_id) {
      let _ = connection.sender.send(ServerMessage::Reply {
        id: request_id,
        result: result.clone(),
      });
    }
    self.trace.duration(
      "server-request-complete",
      request_started.elapsed(),
      &format!(
        "client_id={client_id} request_id={request_id} type={request_label} result={}",
        if result.is_ok() { "ok" } else { "error" }
      ),
    );
    if detach {
      if let Some(connection) = self.clients.get(&client_id) {
        let _ = connection.sender.send(ServerMessage::Detached);
      }
      self.disconnect_client(client_id)?;
    }
    if self.sessions.is_empty() {
      self.should_stop = true;
    }
    Ok(())
  }

  fn send_input(&mut self, number: &str, bytes: &[u8]) -> Result<(), String> {
    let session = self
      .session_mut(number)
      .ok_or_else(|| "Session no longer exists".to_owned())?;
    let tab_id = session
      .model
      .active_tab()
      .ok_or_else(|| "Session has no active Tab".to_owned())?;
    session
      .tabs
      .get_mut(&tab_id)
      .ok_or_else(|| "active Tab process is unavailable".to_owned())?
      .pane
      .send(bytes)
      .map_err(|error| error.to_string())
  }

  fn create_tab(&mut self, number: &str) -> Result<(), String> {
    let (working_directory, pty_size, shell) = {
      let session = self
        .session(number)
        .ok_or_else(|| "Session no longer exists".to_owned())?;
      let workspace = session
        .model
        .active_workspace()
        .ok_or_else(|| "Session has no active Workspace".to_owned())?;
      (workspace.working_directory().to_path_buf(), session.last_pty_size, session.shell.clone())
    };
    let id = TabId::new(self.allocate_tab_id().map_err(|error| error.to_string())?);
    let pane = Pane::shell(
      pty_size.rows,
      pty_size.columns,
      id.get(),
      &working_directory,
      &shell.executable,
      &shell.environment,
      self.pty_events.clone(),
      self.trace.clone(),
    )
    .map_err(|error| error.to_string())?;
    let session = self
      .session_mut(number)
      .ok_or_else(|| "Session no longer exists".to_owned())?;
    if !session.model.add_tab_to_active_workspace(id) {
      return Err("could not add Tab to active Workspace".to_owned());
    }
    let title = pane
      .foreground_process_name()
      .unwrap_or_else(|| format!("Terminal {}", id.get()));
    session.tabs.insert(id, ServerTab {
      id,
      name: next_tab_name(session),
      pane,
      title,
      explicit_title: false,
      exit_code: None,
    });
    self.broadcast_state(number)
  }

  fn close_active_tab(&mut self, number: &str) -> Result<(), String> {
    let tab_id = self
      .session(number)
      .and_then(|session| session.model.active_tab())
      .ok_or_else(|| "Session has no active Tab".to_owned())?;
    self.close_tab(number, tab_id)
  }

  fn close_tab(&mut self, number: &str, tab_id: TabId) -> Result<(), String> {
    let outcome = self
      .session_mut(number)
      .ok_or_else(|| "Session no longer exists".to_owned())?
      .model
      .close_tab(tab_id);
    if matches!(outcome, SessionTabCloseOutcome::NotFound) {
      return Err("Tab was not found in its Session".to_owned());
    }
    if let Some(session) = self.session_mut(number) {
      session.tabs.remove(&tab_id);
    }
    if outcome == SessionTabCloseOutcome::SessionClosed {
      return self
        .close_session(number)
        .map_err(|error| error.to_string());
    }
    self.broadcast_state(number)
  }

  fn select_tab(&mut self, number: &str, one_based_index: usize) -> Result<(), String> {
    let tab_id = self
      .session(number)
      .and_then(|session| session.model.active_workspace())
      .and_then(|workspace| {
        workspace
          .tabs()
          .get(one_based_index.checked_sub(1)?)
          .copied()
      })
      .ok_or_else(|| "Tab number does not exist in the active Workspace".to_owned())?;
    self
      .session_mut(number)
      .ok_or_else(|| "Session no longer exists".to_owned())?
      .model
      .select_tab(tab_id);
    self.broadcast_selection_state(number)
  }

  fn create_workspace(
    &mut self,
    number: &str,
    working_directory: PathBuf,
    name: Option<String>,
  ) -> Result<(), String> {
    if let Some(name) = &name {
      Self::validate_name(name)?;
    }
    if !working_directory.is_dir() {
      return Err(format!("working directory does not exist: {}", working_directory.display()));
    }
    let (size, shell, name) = {
      let session = self
        .session(number)
        .ok_or_else(|| "Session no longer exists".to_owned())?;
      let name = name.unwrap_or_else(|| next_workspace_name(&session.model, &working_directory));
      if session
        .model
        .workspaces()
        .iter()
        .any(|workspace| workspace.name() == name)
      {
        return Err(format!("Workspace {name} already exists"));
      }
      (session.last_pty_size, session.shell.clone(), name)
    };
    let tab_id = TabId::new(self.allocate_tab_id().map_err(|error| error.to_string())?);
    let pane = Pane::shell(
      size.rows,
      size.columns,
      tab_id.get(),
      &working_directory,
      &shell.executable,
      &shell.environment,
      self.pty_events.clone(),
      self.trace.clone(),
    )
    .map_err(|error| error.to_string())?;
    let workspace_id = WorkspaceId::new(
      self
        .allocate_workspace_id()
        .map_err(|error| error.to_string())?,
    );
    let mut workspace = Workspace::new(workspace_id, working_directory, Some(name));
    workspace.add_tab(tab_id);
    let session = self
      .session_mut(number)
      .ok_or_else(|| "Session no longer exists".to_owned())?;
    session
      .model
      .add_workspace(workspace)
      .map_err(|error| error.to_string())?;
    let title = pane
      .foreground_process_name()
      .unwrap_or_else(|| format!("Terminal {}", tab_id.get()));
    session.tabs.insert(tab_id, ServerTab {
      id: tab_id,
      name: "1".to_owned(),
      pane,
      title,
      explicit_title: false,
      exit_code: None,
    });
    self.broadcast_state(number)
  }

  fn edit_workspace(
    &mut self,
    number: &str,
    id: u64,
    working_directory: PathBuf,
    name: Option<String>,
  ) -> Result<(), String> {
    if !working_directory.is_dir() {
      return Err(format!("working directory does not exist: {}", working_directory.display()));
    }
    let workspace = self
      .session_mut(number)
      .and_then(|session| session.model.workspace_mut(WorkspaceId::new(id)))
      .ok_or_else(|| "Workspace does not exist in the Session".to_owned())?;
    workspace.set_working_directory(working_directory);
    if let Some(name) = name {
      workspace.set_name(Some(name));
    }
    self.broadcast_state(number)
  }

  fn select_workspace(&mut self, number: &str, id: u64) -> Result<(), String> {
    if !self
      .session_mut(number)
      .is_some_and(|session| session.model.select_workspace(WorkspaceId::new(id)))
    {
      return Err("Workspace does not exist or is already active".to_owned());
    }
    self.broadcast_selection_state(number)
  }

  fn close_active_workspace(&mut self, number: &str) -> Result<(), String> {
    let (workspace_id, tab_ids) = self
      .session(number)
      .and_then(|session| {
        session
          .model
          .active_workspace()
          .map(|workspace| (workspace.id(), workspace.tabs().to_vec()))
      })
      .ok_or_else(|| "Session has no active Workspace".to_owned())?;
    let outcome = self
      .session_mut(number)
      .ok_or_else(|| "Session no longer exists".to_owned())?
      .model
      .close_workspace(workspace_id);
    if matches!(outcome, SessionTabCloseOutcome::NotFound) {
      return Err("active Workspace was not found in its Session".to_owned());
    }
    if let Some(session) = self.session_mut(number) {
      for tab_id in tab_ids {
        session.tabs.remove(&tab_id);
      }
    }
    if outcome == SessionTabCloseOutcome::SessionClosed {
      return self
        .close_session(number)
        .map_err(|error| error.to_string());
    }
    self.broadcast_state(number)
  }

  fn session_summaries(&self) -> Vec<SessionSummary> {
    self
      .sessions
      .iter()
      .map(|session| SessionSummary {
        number: session.model.name().get().to_owned(),
        clients: session.clients.len(),
        workspaces: session.model.workspaces().len(),
        tabs: session.tabs.len(),
      })
      .collect()
  }

  fn switch_client_session(&mut self, client_id: u64, target: &str) -> Result<(), String> {
    let current = self
      .clients
      .get(&client_id)
      .ok_or_else(|| "Client is no longer connected".to_owned())?
      .session_name
      .clone();
    if current == target {
      return Ok(());
    }
    if self.session(target).is_none() {
      return Err(format!("Session {target} does not exist"));
    }

    if let Some(session) = self.session_mut(&current) {
      session.clients.remove(&client_id);
    }
    if let Some(connection) = self.clients.get_mut(&client_id) {
      connection.session_name = target.to_owned();
    }
    if let Some(session) = self.session_mut(target) {
      session.clients.insert(client_id);
    }

    self
      .resize_session(&current)
      .map_err(|error| error.to_string())?;
    self
      .resize_session(target)
      .map_err(|error| error.to_string())?;
    if let Some(connection) = self.clients.get(&client_id)
      && let Some(session) = self.session(target)
    {
      connection
        .sender
        .send(ServerMessage::StateChanged {
          session: session.snapshot(),
          terminals: session.terminal_snapshots(),
        })
        .map_err(|error| error.to_string())?;
    }
    self
      .broadcast_state(&current)
      .map_err(|error| error.to_string())?;
    self
      .broadcast_state(target)
      .map_err(|error| error.to_string())?;
    Ok(())
  }

  fn close_session(&mut self, number: &str) -> Result<(), Box<dyn std::error::Error>> {
    let Some(index) = self
      .sessions
      .iter()
      .position(|session| session.model.name().get() == number)
    else {
      return Ok(());
    };
    let removed = self.sessions.remove(index);
    let affected_clients: Vec<u64> = removed.clients.iter().copied().collect();
    drop(removed);

    if self.sessions.is_empty() {
      for client_id in affected_clients {
        if let Some(client) = self.clients.remove(&client_id) {
          let _ = client.sender.send(ServerMessage::SessionClosed);
        }
      }
      self.should_stop = true;
      return Ok(());
    }

    let next_index = index % self.sessions.len();
    let next_number = self.sessions[next_index].model.name().get().to_owned();
    for client_id in affected_clients {
      if let Some(client) = self.clients.get_mut(&client_id) {
        client.session_name = next_number.clone();
        self.sessions[next_index].clients.insert(client_id);
      }
    }
    self.resize_session(&next_number)?;
    self
      .broadcast_state(&next_number)
      .map_err(io::Error::other)?;
    Ok(())
  }

  fn stop_server(&mut self) {
    for client in self.clients.values() {
      let _ = client.sender.send(ServerMessage::ServerStopping);
    }
    self.clients.clear();
    self.sessions.clear();
    self.should_stop = true;
  }

  fn disconnect_client(&mut self, client_id: u64) -> Result<(), Box<dyn std::error::Error>> {
    let Some(client) = self.clients.remove(&client_id) else {
      return Ok(());
    };
    if let Some(session) = self.session_mut(&client.session_name) {
      session.clients.remove(&client_id);
    }
    let previous_size = self
      .session(&client.session_name)
      .map(|session| session.last_pty_size);
    self.resize_session(&client.session_name)?;
    if self
      .session(&client.session_name)
      .is_some_and(|session| Some(session.last_pty_size) != previous_size)
    {
      let _ = self.broadcast_state(&client.session_name);
    }
    Ok(())
  }

  fn handle_pty_output(&mut self, tab_id: u64, bytes: Vec<u8>) {
    let Some((number, events)) = self.sessions.iter_mut().find_map(|session| {
      let tab = session
        .tabs
        .values_mut()
        .find(|tab| tab.id.get() == tab_id)?;
      let events = tab.pane.process_output(&bytes).ok()?;
      if let Some(title) = events.title.as_ref() {
        tab.explicit_title = !title.is_empty();
        tab.title = if title.is_empty() {
          tab
            .pane
            .foreground_process_name()
            .unwrap_or_else(|| format!("Terminal {}", tab.id.get()))
        } else {
          title.clone()
        };
      }
      Some((session.model.name().get().to_owned(), events))
    }) else {
      return;
    };
    if let Some(session) = self.session(&number) {
      for client_id in &session.clients {
        if let Some(client) = self.clients.get(client_id) {
          let _ = client.sender.send(ServerMessage::TerminalOutput {
            tab_id,
            bytes: bytes.clone(),
          });
          for text in &events.clipboard_stores {
            let _ = client.sender.send(ServerMessage::ClipboardStore {
              tab_id,
              text: text.clone(),
            });
          }
        }
      }
    }
  }

  fn poll_processes(&mut self) -> Result<(), Box<dyn std::error::Error>> {
    let mut changed_sessions = HashSet::new();
    let mut closed_tabs = Vec::new();
    for session in &mut self.sessions {
      let number = session.model.name().get().to_owned();
      for tab in session.tabs.values_mut() {
        if !tab.explicit_title
          && let Some(process_name) = tab.pane.foreground_process_name()
          && tab.title != process_name
        {
          tab.title = process_name;
          changed_sessions.insert(number.clone());
        }
        if let Some(exit_code) = tab.pane.poll_exit()? {
          tab.exit_code = Some(exit_code);
          closed_tabs.push((number.clone(), tab.id));
        }
      }
    }
    for (number, tab_id) in closed_tabs {
      if self.session(&number).is_some() {
        self.close_tab(&number, tab_id).map_err(io::Error::other)?;
      }
    }
    for number in changed_sessions {
      if self.session(&number).is_some() {
        self.broadcast_state(&number).map_err(io::Error::other)?;
      }
    }
    Ok(())
  }

  fn resize_session(&mut self, number: &str) -> io::Result<()> {
    let max_size = self.session(number).and_then(|session| {
      session
        .clients
        .iter()
        .filter_map(|client_id| self.clients.get(client_id))
        .map(|client| client.viewport)
        .reduce(|left, right| Viewport {
          rows: left.rows.max(right.rows),
          columns: left.columns.max(right.columns),
        })
    });
    let Some(size) = max_size else {
      return Ok(());
    };
    let size = sanitize_viewport(size);
    let Some(session) = self.session_mut(number) else {
      return Ok(());
    };
    if session.last_pty_size == size {
      return Ok(());
    }
    for tab in session.tabs.values_mut() {
      tab.pane.resize(size.rows, size.columns)?;
    }
    session.last_pty_size = size;
    Ok(())
  }

  fn broadcast_state(&self, number: &str) -> Result<(), String> {
    let session = self
      .session(number)
      .ok_or_else(|| "Session no longer exists".to_owned())?;
    let snapshot_started = Instant::now();
    let state = session.snapshot();
    let state_snapshot_duration = snapshot_started.elapsed();
    let terminal_snapshot_started = Instant::now();
    let terminals = session.terminal_snapshots();
    self.trace.duration(
      "server-state-snapshots",
      snapshot_started.elapsed(),
      &format!(
        "state={state_snapshot_duration:?} terminals={:?} terminal_count={}",
        terminal_snapshot_started.elapsed(),
        terminals.len()
      ),
    );
    let broadcast_started = Instant::now();
    for client_id in &session.clients {
      if let Some(client) = self.clients.get(client_id) {
        let _ = client.sender.send(ServerMessage::StateChanged {
          session: state.clone(),
          terminals: terminals.clone(),
        });
      }
    }
    self.trace.duration(
      "server-state-broadcast-queued",
      broadcast_started.elapsed(),
      &format!("clients={} terminals={}", session.clients.len(), terminals.len()),
    );
    Ok(())
  }

  fn broadcast_selection_state(&self, number: &str) -> Result<(), String> {
    let session = self
      .session(number)
      .ok_or_else(|| "Session no longer exists".to_owned())?;
    let snapshot_started = Instant::now();
    let state = session.snapshot();
    self.trace.duration(
      "server-selection-state-snapshot",
      snapshot_started.elapsed(),
      &format!("workspaces={}", state.workspaces.len()),
    );
    let broadcast_started = Instant::now();
    for client_id in &session.clients {
      if let Some(client) = self.clients.get(client_id) {
        let _ = client.sender.send(ServerMessage::StateChanged {
          session: state.clone(),
          terminals: Vec::new(),
        });
      }
    }
    self.trace.duration(
      "server-selection-state-broadcast-queued",
      broadcast_started.elapsed(),
      &format!("clients={}", session.clients.len()),
    );
    Ok(())
  }

  fn session(&self, number: &str) -> Option<&ServerSession> {
    self
      .sessions
      .iter()
      .find(|session| session.model.name().get() == number)
  }

  fn session_mut(&mut self, number: &str) -> Option<&mut ServerSession> {
    self
      .sessions
      .iter_mut()
      .find(|session| session.model.name().get() == number)
  }

  fn next_session_name(&self) -> String {
    let mut number = 1_u64;
    loop {
      let candidate = number.to_string();
      if !self
        .sessions
        .iter()
        .any(|session| session.model.name().get() == candidate)
      {
        return candidate;
      }
      number = number.saturating_add(1);
    }
  }

  fn allocate_workspace_id(&mut self) -> Result<u64, io::Error> {
    allocate_id(&mut self.next_workspace_id)
  }

  fn allocate_tab_id(&mut self) -> Result<u64, io::Error> { allocate_id(&mut self.next_tab_id) }
}

impl ServerSession {
  fn snapshot(&self) -> SessionSnapshot {
    let workspaces = self
      .model
      .workspaces()
      .iter()
      .map(|workspace| WorkspaceSnapshot {
        id: workspace.id().get(),
        name: workspace.name().into_owned(),
        explicit_name: workspace.explicit_name().map(str::to_owned),
        working_directory: workspace.working_directory().to_path_buf(),
        tabs: workspace
          .tabs()
          .iter()
          .filter_map(|id| self.tabs.get(id))
          .map(|tab| WireTabSnapshot {
            id: tab.id.get(),
            name: tab.name.clone(),
            title: tab.title.clone(),
            exit_code: tab.exit_code,
          })
          .collect(),
        active_tab_id: workspace.active_tab().map_or(0, TabId::get),
      })
      .collect();
    SessionSnapshot {
      number: self.model.name().get().to_owned(),
      workspaces,
      active_workspace_id: self.model.active_workspace_id().map_or(0, WorkspaceId::get),
    }
  }

  fn terminal_snapshots(&self) -> Vec<TerminalSnapshot> {
    self
      .tabs
      .values()
      .map(|tab| {
        let (rows, columns) = tab.pane.size();
        TerminalSnapshot {
          tab_id: tab.id.get(),
          rows,
          columns,
          bytes: tab.pane.replay().to_vec(),
          title: tab.title.clone(),
          exit_code: tab.exit_code,
        }
      })
      .collect()
  }
}

fn sanitize_viewport(viewport: Viewport) -> Viewport {
  Viewport {
    rows: viewport.rows.max(1),
    columns: viewport.columns.max(1),
  }
}

fn next_workspace_name(session: &Session, working_directory: &std::path::Path) -> String {
  let used: HashSet<String> = session
    .workspaces()
    .iter()
    .map(|workspace| workspace.name().into_owned())
    .collect();
  let base = working_directory
    .file_name()
    .unwrap_or(working_directory.as_os_str())
    .to_string_lossy()
    .into_owned();
  if !used.contains(&base) {
    return base;
  }

  let mut number = 1_u64;
  loop {
    let candidate = format!("{base}{number}");
    if !used.contains(&candidate) {
      return candidate;
    }
    number = number.saturating_add(1);
  }
}

fn next_tab_name(session: &ServerSession) -> String {
  let used: HashSet<&str> = session
    .model
    .active_workspace()
    .into_iter()
    .flat_map(|workspace| {
      workspace
        .tabs()
        .iter()
        .filter_map(|id| session.tabs.get(id).map(|tab| tab.name.as_str()))
    })
    .collect();
  let mut number = 1_u64;
  loop {
    let candidate = number.to_string();
    if !used.contains(candidate.as_str()) {
      return candidate;
    }
    number = number.saturating_add(1);
  }
}

fn allocate_id(next: &mut u64) -> io::Result<u64> {
  let value = *next;
  *next = next
    .checked_add(1)
    .ok_or_else(|| io::Error::other("runtime identifier space exhausted"))?;
  Ok(value)
}

#[cfg(test)]
mod tests {
  use std::{
    thread,
    time::Duration,
  };

  use zmux::{
    protocol::{
      AdminRequest,
      AdminResponse,
      ClientHello,
      ClientMessage,
      ClientRequest,
      HeadlessHello,
      HeadlessRequest,
      HeadlessResult,
      ServerMessage,
      SessionSnapshot,
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

  use super::run;

  fn connect_test_client(
    endpoint: &Endpoint,
    session_name: Option<String>,
    viewport: Viewport,
  ) -> (
    interprocess::local_socket::prelude::LocalSocketStream,
    SessionSnapshot,
    Vec<TerminalSnapshot>,
  ) {
    let mut client = None;
    for _ in 0..100 {
      match transport::connect(endpoint) {
        Ok(stream) => {
          client = Some(stream);
          break;
        }
        Err(_) => thread::sleep(Duration::from_millis(10)),
      }
    }
    let mut client = client.expect("connect test Client");
    write_frame(
      &mut client,
      &ClientMessage::Hello(ClientHello {
        protocol_version: zmux::protocol::PROTOCOL_VERSION,
        session_name,
        create_if_missing: false,
        fail_if_exists: false,
        shell: "/bin/sh".into(),
        environment: vec![("SHELL".to_owned(), "/bin/sh".to_owned())],
        working_directory: std::env::current_dir().expect("current directory"),
        viewport,
      }),
    )
    .expect("send hello");
    assert!(matches!(
      read_frame::<_, ServerMessage>(&mut client).expect("read acceptance"),
      ServerMessage::HelloAccepted { .. }
    ));
    let ServerMessage::InitialState { session, terminals } =
      read_frame::<_, ServerMessage>(&mut client).expect("read initial state")
    else {
      panic!("expected initial Session state");
    };
    (client, session, terminals)
  }

  fn read_next_state(
    client: &mut interprocess::local_socket::prelude::LocalSocketStream,
  ) -> SessionSnapshot {
    loop {
      match read_frame::<_, ServerMessage>(client).expect("read server update") {
        ServerMessage::StateChanged { session, .. } => return session,
        ServerMessage::TerminalOutput { .. }
        | ServerMessage::TerminalChanged(_)
        | ServerMessage::Reply { .. } => {}
        message => panic!("unexpected server update: {message:?}"),
      }
    }
  }

  fn read_state_with_workspace_count(
    client: &mut interprocess::local_socket::prelude::LocalSocketStream,
    count: usize,
  ) -> SessionSnapshot {
    loop {
      let state = read_next_state(client);
      if state.workspaces.len() == count {
        return state;
      }
    }
  }

  fn read_state_with_active_workspace(
    client: &mut interprocess::local_socket::prelude::LocalSocketStream,
    workspace_id: u64,
  ) -> SessionSnapshot {
    loop {
      let state = read_next_state(client);
      if state.active_workspace_id == workspace_id {
        return state;
      }
    }
  }

  fn read_state_with_session_name(
    client: &mut interprocess::local_socket::prelude::LocalSocketStream,
    number: &str,
  ) -> SessionSnapshot {
    loop {
      let state = read_next_state(client);
      if state.number == number {
        return state;
      }
    }
  }

  fn read_state_with_workspace_directory(
    client: &mut interprocess::local_socket::prelude::LocalSocketStream,
    workspace_id: u64,
    directory: &std::path::Path,
  ) -> SessionSnapshot {
    loop {
      let state = read_next_state(client);
      if state
        .workspaces
        .iter()
        .any(|workspace| workspace.id == workspace_id && workspace.working_directory == directory)
      {
        return state;
      }
    }
  }

  fn read_state_with_active_tab_count(
    client: &mut interprocess::local_socket::prelude::LocalSocketStream,
    tab_count: usize,
  ) -> SessionSnapshot {
    loop {
      let state = read_next_state(client);
      if state
        .workspaces
        .iter()
        .find(|workspace| workspace.id == state.active_workspace_id)
        .is_some_and(|workspace| workspace.tabs.len() == tab_count)
      {
        return state;
      }
    }
  }

  fn read_until_terminal_output(
    client: &mut interprocess::local_socket::prelude::LocalSocketStream,
    tab_id: u64,
    expected: &str,
  ) {
    let mut output = Vec::new();
    loop {
      match read_frame::<_, ServerMessage>(client).expect("read terminal output") {
        ServerMessage::TerminalOutput {
          tab_id: output_tab,
          bytes,
        } if output_tab == tab_id => {
          output.extend(bytes);
          if String::from_utf8_lossy(&output).contains(expected) {
            return;
          }
        }
        ServerMessage::StateChanged { .. }
        | ServerMessage::TerminalChanged(_)
        | ServerMessage::Reply { .. }
        | ServerMessage::TerminalOutput { .. } => {}
        message => panic!("unexpected terminal message: {message:?}"),
      }
    }
  }

  fn assert_server_stopping(client: &mut interprocess::local_socket::prelude::LocalSocketStream) {
    loop {
      if matches!(
        read_frame::<_, ServerMessage>(client).expect("read stop notification"),
        ServerMessage::ServerStopping
      ) {
        break;
      }
    }
  }

  #[test]
  fn underscore_input_is_written_to_the_shell_pty_unchanged() {
    let endpoint = Endpoint::new(Some(&format!("underscore-pty-test-{}", std::process::id())))
      .expect("test endpoint");
    let server_endpoint = endpoint.clone();
    let server = thread::spawn(move || run(server_endpoint).expect("server loop"));
    let (mut client, state, _) = connect_test_client(&endpoint, None, Viewport {
      rows: 24,
      columns: 80,
    });
    let tab_id = state.workspaces[0].tabs[0].id;

    write_frame(&mut client, &ClientMessage::Request {
      id: 1,
      request: ClientRequest::Input(b"printf '__ZMUX_UNDERSCORE_42__\\n'\r".to_vec()),
    })
    .expect("send underscore input through the Server to the PTY");
    read_until_terminal_output(&mut client, tab_id, "__ZMUX_UNDERSCORE_42__");

    write_frame(&mut client, &ClientMessage::Request {
      id: 2,
      request: ClientRequest::StopServer,
    })
    .expect("stop Server");
    assert_server_stopping(&mut client);
    drop(client);
    server.join().expect("Server stops cleanly");
  }

  #[test]
  fn tab_selection_broadcasts_state_without_terminal_snapshots() {
    let endpoint = Endpoint::new(Some(&format!("selection-state-test-{}", std::process::id())))
      .expect("test endpoint");
    let server_endpoint = endpoint.clone();
    let server = thread::spawn(move || run(server_endpoint).expect("server loop"));
    let (mut client, _, _) = connect_test_client(&endpoint, None, Viewport {
      rows: 24,
      columns: 80,
    });

    write_frame(&mut client, &ClientMessage::Request {
      id: 1,
      request: ClientRequest::NewTab,
    })
    .expect("create second Tab");
    let state = read_state_with_active_tab_count(&mut client, 2);
    let first_tab = state.workspaces[0].tabs[0].id;

    write_frame(&mut client, &ClientMessage::Request {
      id: 2,
      request: ClientRequest::SelectTab(1),
    })
    .expect("select first Tab");
    let terminals = loop {
      match read_frame::<_, ServerMessage>(&mut client).expect("read Tab selection update") {
        ServerMessage::StateChanged { session, terminals } => {
          assert_eq!(session.workspaces[0].active_tab_id, first_tab);
          break terminals;
        }
        ServerMessage::TerminalOutput { .. }
        | ServerMessage::TerminalChanged(_)
        | ServerMessage::Reply { .. } => {}
        message => panic!("unexpected Tab selection update: {message:?}"),
      }
    };
    assert!(terminals.is_empty(), "tab selection must not resend terminal contents");

    write_frame(&mut client, &ClientMessage::Request {
      id: 3,
      request: ClientRequest::StopServer,
    })
    .expect("stop Server");
    assert_server_stopping(&mut client);
    drop(client);
    server.join().expect("Server stops cleanly");
  }

  // Developer approved: do not remove.
  #[test]
  fn attached_clients_share_workspace_changes_and_closing_cascades() {
    let endpoint = Endpoint::new(Some(&format!("multi-client-test-{}", std::process::id())))
      .expect("test endpoint");
    let server_endpoint = endpoint.clone();
    let server = thread::spawn(move || run(server_endpoint).expect("server loop"));
    let (mut first, first_state, first_terminals) =
      connect_test_client(&endpoint, None, Viewport {
        rows: 24,
        columns: 80,
      });
    assert_eq!(first_state.number, "1");
    let (mut second, second_state, second_terminals) =
      connect_test_client(&endpoint, Some("1".to_owned()), Viewport {
        rows: 40,
        columns: 120,
      });
    assert_eq!(second_state.number, "1");
    assert_eq!((first_terminals[0].rows, first_terminals[0].columns), (24, 80));
    assert_eq!((second_terminals[0].rows, second_terminals[0].columns), (40, 120));
    let (mut third, third_state, _) = connect_test_client(&endpoint, None, Viewport {
      rows: 24,
      columns: 80,
    });
    assert_eq!(third_state.number, "2", "a no-target launch creates a new Session");

    write_frame(&mut first, &ClientMessage::Request {
      id: 2,
      request: ClientRequest::CreateWorkspace {
        working_directory: std::env::current_dir().expect("current directory"),
        name: Some("shared".to_owned()),
      },
    })
    .expect("create Workspace");
    let first_update = read_state_with_workspace_count(&mut first, 2);
    let second_update = read_state_with_workspace_count(&mut second, 2);
    assert_eq!(first_update, second_update);
    assert_eq!(first_update.workspaces.len(), 2);
    assert_eq!(first_update.workspaces[first_update.workspaces.len() - 1].name, "shared");
    assert_eq!(
      first_update.active_workspace_id,
      first_update.workspaces.last().expect("new Workspace").id
    );

    let original_workspace_id = first_state.workspaces[0].id;
    let active_workspace_id = first_update.active_workspace_id;
    write_frame(&mut second, &ClientMessage::Request {
      id: 3,
      request: ClientRequest::SelectWorkspace(original_workspace_id),
    })
    .expect("switch shared Workspace");
    let first_selection = read_state_with_active_workspace(&mut first, original_workspace_id);
    let second_selection = read_state_with_active_workspace(&mut second, original_workspace_id);
    assert_eq!(first_selection, second_selection);
    assert_eq!(first_selection.active_workspace_id, original_workspace_id);

    write_frame(&mut first, &ClientMessage::Request {
      id: 4,
      request: ClientRequest::SelectWorkspace(active_workspace_id),
    })
    .expect("switch back to the new Workspace");
    let _ = read_state_with_active_workspace(&mut first, active_workspace_id);
    let _ = read_state_with_active_workspace(&mut second, active_workspace_id);

    let active_tab = first_update
      .workspaces
      .last()
      .and_then(|workspace| workspace.tabs.first())
      .expect("new Workspace starts with a Tab")
      .id;
    write_frame(&mut second, &ClientMessage::Request {
      id: 5,
      request: ClientRequest::CloseTab,
    })
    .expect("close active Tab");
    let after_close_first = read_state_with_workspace_count(&mut first, 1);
    let after_close_second = read_state_with_workspace_count(&mut second, 1);
    assert_eq!(after_close_first, after_close_second);
    assert_eq!(after_close_first.workspaces.len(), 1);
    assert_ne!(after_close_first.active_workspace_id, first_update.active_workspace_id);
    assert!(
      !after_close_first
        .workspaces
        .iter()
        .flat_map(|workspace| &workspace.tabs)
        .any(|tab| tab.id == active_tab)
    );

    write_frame(&mut first, &ClientMessage::Request {
      id: 6,
      request: ClientRequest::KillSession,
    })
    .expect("kill Session");
    assert_eq!(read_state_with_session_name(&mut first, "2").number, "2");
    assert_eq!(read_state_with_session_name(&mut second, "2").number, "2");

    write_frame(&mut first, &ClientMessage::Request {
      id: 7,
      request: ClientRequest::StopServer,
    })
    .expect("stop Server");
    assert_server_stopping(&mut first);
    assert_server_stopping(&mut second);
    assert_server_stopping(&mut third);
    drop(first);
    drop(second);
    drop(third);
    server.join().expect("Server stops cleanly");
  }

  #[cfg(unix)]
  // Developer approved: do not remove.
  #[test]
  fn workspace_directory_changes_only_affect_new_tabs() {
    let root = std::env::temp_dir().join(format!("zmux-cwd-test-{}", std::process::id()));
    let first_directory = root.join("first");
    let second_directory = root.join("second");
    std::fs::create_dir_all(&first_directory).expect("create first test directory");
    std::fs::create_dir_all(&second_directory).expect("create second test directory");

    let endpoint =
      Endpoint::new(Some(&format!("cwd-test-{}", std::process::id()))).expect("test endpoint");
    let server_endpoint = endpoint.clone();
    let server = thread::spawn(move || run(server_endpoint).expect("server loop"));
    let (mut client, _, _) = connect_test_client(&endpoint, None, Viewport {
      rows: 24,
      columns: 80,
    });

    write_frame(&mut client, &ClientMessage::Request {
      id: 1,
      request: ClientRequest::CreateWorkspace {
        working_directory: first_directory.clone(),
        name: None,
      },
    })
    .expect("create Workspace");
    let state = read_state_with_workspace_count(&mut client, 2);
    let workspace_id = state.active_workspace_id;
    let original_tab_id = state
      .workspaces
      .iter()
      .find(|workspace| workspace.id == workspace_id)
      .and_then(|workspace| workspace.tabs.first())
      .expect("new Workspace has a Tab")
      .id;
    assert_eq!(
      state
        .workspaces
        .iter()
        .find(|workspace| workspace.id == workspace_id)
        .expect("new active Workspace")
        .name,
      "first"
    );
    write_frame(&mut client, &ClientMessage::Request {
      id: 2,
      request: ClientRequest::Input(b"pwd\r".to_vec()),
    })
    .expect("run pwd in first Tab");
    read_until_terminal_output(&mut client, original_tab_id, &first_directory.to_string_lossy());

    write_frame(&mut client, &ClientMessage::Request {
      id: 3,
      request: ClientRequest::EditWorkspace {
        id: workspace_id,
        working_directory: second_directory.clone(),
        name: None,
      },
    })
    .expect("edit Workspace directory");
    let edited_state =
      read_state_with_workspace_directory(&mut client, workspace_id, &second_directory);
    assert_eq!(
      edited_state
        .workspaces
        .iter()
        .find(|workspace| workspace.id == workspace_id)
        .expect("edited Workspace")
        .name,
      "first"
    );
    write_frame(&mut client, &ClientMessage::Request {
      id: 4,
      request: ClientRequest::NewTab,
    })
    .expect("create Tab after directory edit");
    let state = read_state_with_active_tab_count(&mut client, 2);
    let new_tab_id = state
      .workspaces
      .iter()
      .find(|workspace| workspace.id == workspace_id)
      .and_then(|workspace| {
        workspace
          .tabs
          .iter()
          .find(|tab| tab.id == workspace.active_tab_id)
      })
      .expect("new active Tab")
      .id;
    assert_ne!(new_tab_id, original_tab_id);
    write_frame(&mut client, &ClientMessage::Request {
      id: 5,
      request: ClientRequest::Input(b"pwd\r".to_vec()),
    })
    .expect("run pwd in new Tab");
    read_until_terminal_output(&mut client, new_tab_id, &second_directory.to_string_lossy());

    write_frame(&mut client, &ClientMessage::Request {
      id: 6,
      request: ClientRequest::CreateWorkspace {
        working_directory: first_directory.clone(),
        name: None,
      },
    })
    .expect("create duplicate Workspace");
    let duplicate_state = read_state_with_workspace_count(&mut client, 3);
    assert_eq!(
      duplicate_state
        .workspaces
        .iter()
        .find(|workspace| workspace.id == duplicate_state.active_workspace_id)
        .expect("new duplicate Workspace")
        .name,
      "first1"
    );

    write_frame(&mut client, &ClientMessage::Request {
      id: 7,
      request: ClientRequest::CreateWorkspace {
        working_directory: first_directory,
        name: None,
      },
    })
    .expect("create second duplicate Workspace");
    let second_duplicate_state = read_state_with_workspace_count(&mut client, 4);
    assert_eq!(
      second_duplicate_state
        .workspaces
        .iter()
        .find(|workspace| workspace.id == second_duplicate_state.active_workspace_id)
        .expect("new second duplicate Workspace")
        .name,
      "first2"
    );

    write_frame(&mut client, &ClientMessage::Request {
      id: 8,
      request: ClientRequest::StopServer,
    })
    .expect("stop Server");
    assert_server_stopping(&mut client);
    drop(client);
    server.join().expect("Server stops cleanly");
    std::fs::remove_dir_all(root).expect("remove test directories");
  }

  #[test]
  fn admin_prepares_a_session_before_client_attachment() {
    let endpoint = Endpoint::new(Some(&format!("prepare-session-test-{}", std::process::id())))
      .expect("test endpoint");
    let server_endpoint = endpoint.clone();
    let server = thread::spawn(move || run(server_endpoint).expect("server loop"));
    let mut admin = connect_for_hello(&endpoint);
    let request = || AdminRequest::PrepareSession {
      session_name: Some("prepared".to_owned()),
      create_if_missing: true,
      fail_if_exists: false,
      working_directory: std::env::current_dir().expect("current directory"),
      shell: "/bin/sh".into(),
      environment: vec![("SHELL".to_owned(), "/bin/sh".to_owned())],
      viewport: Viewport {
        rows: 24,
        columns: 80,
      },
    };
    write_frame(&mut admin, &ClientMessage::Admin(request())).expect("prepare Session");
    assert!(matches!(
      read_frame::<_, ServerMessage>(&mut admin).expect("read prepared Session response"),
      ServerMessage::AdminResult {
        result: Ok(AdminResponse::SessionReady { ref name, created: true }),
      } if name == "prepared"
    ));
    drop(admin);

    let mut attach = connect_for_hello(&endpoint);
    write_frame(
      &mut attach,
      &ClientMessage::Hello(ClientHello {
        protocol_version: zmux::protocol::PROTOCOL_VERSION,
        session_name: Some("prepared".to_owned()),
        create_if_missing: false,
        fail_if_exists: false,
        shell: "/bin/sh".into(),
        environment: vec![("SHELL".to_owned(), "/bin/sh".to_owned())],
        working_directory: std::env::current_dir().expect("current directory"),
        viewport: Viewport {
          rows: 24,
          columns: 80,
        },
      }),
    )
    .expect("attach prepared Session");
    let mut accepted = false;
    let mut initial_state = false;
    while !initial_state {
      match read_frame::<_, ServerMessage>(&mut attach).expect("read attach handshake") {
        ServerMessage::HelloAccepted { .. } => accepted = true,
        ServerMessage::InitialState { session, .. } => {
          assert!(accepted, "server accepts Client before sending initial state");
          assert_eq!(session.number, "prepared");
          initial_state = true;
        }
        other => panic!("unexpected attach message: {other:?}"),
      }
    }
    drop(attach);

    let mut admin = connect_for_hello(&endpoint);
    write_frame(
      &mut admin,
      &ClientMessage::Admin(AdminRequest::PrepareSession {
        session_name: Some("prepared".to_owned()),
        create_if_missing: true,
        fail_if_exists: true,
        working_directory: std::env::current_dir().expect("current directory"),
        shell: "/bin/sh".into(),
        environment: vec![("SHELL".to_owned(), "/bin/sh".to_owned())],
        viewport: Viewport {
          rows: 24,
          columns: 80,
        },
      }),
    )
    .expect("request strict duplicate");
    assert!(matches!(
      read_frame::<_, ServerMessage>(&mut admin).expect("read strict duplicate response"),
      ServerMessage::AdminResult { result: Err(reason) } if reason.contains("already exists")
    ));
    drop(admin);

    let mut admin = connect_for_hello(&endpoint);
    write_frame(&mut admin, &ClientMessage::Admin(AdminRequest::StopServer))
      .expect("stop prepared Session Server");
    let _ = read_frame::<_, ServerMessage>(&mut admin).expect("read stop response");
    drop(admin);
    server.join().expect("Server stops cleanly");
  }

  // Developer approved: do not remove.
  #[test]
  fn attach_to_unknown_session_name_is_rejected() {
    let endpoint = Endpoint::new(Some(&format!("unknown-session-test-{}", std::process::id())))
      .expect("test endpoint");
    let server_endpoint = endpoint.clone();
    let server = thread::spawn(move || run(server_endpoint).expect("server loop"));
    let mut client = connect_for_hello(&endpoint);
    write_frame(
      &mut client,
      &ClientMessage::Hello(ClientHello {
        protocol_version: zmux::protocol::PROTOCOL_VERSION,
        session_name: Some("42".to_owned()),
        create_if_missing: false,
        fail_if_exists: false,
        shell: "/bin/sh".into(),
        environment: vec![("SHELL".to_owned(), "/bin/sh".to_owned())],
        working_directory: std::env::current_dir().expect("current directory"),
        viewport: Viewport {
          rows: 24,
          columns: 80,
        },
      }),
    )
    .expect("request missing Session");

    let ServerMessage::HelloRejected { reason } =
      read_frame::<_, ServerMessage>(&mut client).expect("read attach error")
    else {
      panic!("missing Session should be rejected");
    };
    assert!(reason.contains("42"));
    drop(client);
    server.join().expect("Server exits without Sessions");
  }

  // Developer approved: do not remove.
  #[test]
  fn incompatible_client_protocol_is_rejected_without_a_session() {
    let endpoint =
      Endpoint::new(Some(&format!("version-test-{}", std::process::id()))).expect("test endpoint");
    let server_endpoint = endpoint.clone();
    let server = thread::spawn(move || run(server_endpoint).expect("server loop"));
    let mut client = connect_for_hello(&endpoint);
    write_frame(
      &mut client,
      &ClientMessage::Hello(ClientHello {
        protocol_version: zmux::protocol::PROTOCOL_VERSION + 1,
        session_name: None,
        create_if_missing: false,
        fail_if_exists: false,
        shell: "/bin/sh".into(),
        environment: vec![("SHELL".to_owned(), "/bin/sh".to_owned())],
        working_directory: std::env::current_dir().expect("current directory"),
        viewport: Viewport {
          rows: 24,
          columns: 80,
        },
      }),
    )
    .expect("send incompatible protocol version");
    assert!(matches!(
      read_frame::<_, ServerMessage>(&mut client).expect("read protocol rejection"),
      ServerMessage::HelloRejected { .. }
    ));
    drop(client);
    server.join().expect("Server exits without Sessions");
  }

  fn connect_for_hello(
    endpoint: &Endpoint,
  ) -> interprocess::local_socket::prelude::LocalSocketStream {
    for _ in 0..100 {
      match transport::connect(endpoint) {
        Ok(stream) => return stream,
        Err(_) => thread::sleep(Duration::from_millis(10)),
      }
    }
    panic!("Server listener becomes ready")
  }

  // Developer approved: do not remove.
  #[test]
  fn closing_the_final_tab_cascades_to_session_and_server_shutdown() {
    let endpoint =
      Endpoint::new(Some(&format!("cascade-test-{}", std::process::id()))).expect("test endpoint");
    let server_endpoint = endpoint.clone();
    let server = thread::spawn(move || run(server_endpoint).expect("server loop"));
    let (mut client, _, _) = connect_test_client(&endpoint, None, Viewport {
      rows: 24,
      columns: 80,
    });

    write_frame(&mut client, &ClientMessage::Request {
      id: 1,
      request: ClientRequest::CloseTab,
    })
    .expect("close final Tab");
    assert_eq!(
      read_frame::<_, ServerMessage>(&mut client).expect("read Session closure"),
      ServerMessage::SessionClosed
    );
    drop(client);
    server
      .join()
      .expect("Server exits after final Session closes");
  }

  // Developer approved: do not remove.
  #[test]
  fn shell_exit_closes_the_tab_and_final_session() {
    let endpoint = Endpoint::new(Some(&format!("shell-exit-test-{}", std::process::id())))
      .expect("test endpoint");
    let server_endpoint = endpoint.clone();
    let server = thread::spawn(move || run(server_endpoint).expect("server loop"));
    let (mut client, initial_state, _) = connect_test_client(&endpoint, None, Viewport {
      rows: 24,
      columns: 80,
    });
    let initial_tab_id = initial_state.workspaces[0].tabs[0].id;

    write_frame(&mut client, &ClientMessage::Request {
      id: 1,
      request: ClientRequest::NewTab,
    })
    .expect("create second Tab");
    let state = read_state_with_active_tab_count(&mut client, 2);
    let exited_tab_id = state
      .workspaces
      .iter()
      .find(|workspace| workspace.id == state.active_workspace_id)
      .expect("active Workspace")
      .active_tab_id;

    write_frame(&mut client, &ClientMessage::Request {
      id: 2,
      request: ClientRequest::Input(b"exit\r".to_vec()),
    })
    .expect("exit shell");
    let state = read_state_with_active_tab_count(&mut client, 1);
    let workspace = state
      .workspaces
      .iter()
      .find(|workspace| workspace.id == state.active_workspace_id)
      .expect("active Workspace");
    assert_eq!(workspace.tabs[0].id, initial_tab_id);
    assert_ne!(workspace.tabs[0].id, exited_tab_id);

    write_frame(&mut client, &ClientMessage::Request {
      id: 3,
      request: ClientRequest::CloseTab,
    })
    .expect("close remaining Tab");
    loop {
      match read_frame::<_, ServerMessage>(&mut client).expect("read final Session closure") {
        ServerMessage::SessionClosed => break,
        ServerMessage::TerminalOutput { .. }
        | ServerMessage::TerminalChanged(_)
        | ServerMessage::Reply { .. } => {}
        message => panic!("unexpected server update: {message:?}"),
      }
    }
    drop(client);
    server.join().expect("Server exits after final Tab closes");
  }

  // Developer approved: do not remove.
  #[test]
  fn no_target_connection_creates_numbered_session_and_stop_destroys_server() {
    let endpoint =
      Endpoint::new(Some(&format!("server-test-{}", std::process::id()))).expect("test endpoint");
    let server_endpoint = endpoint.clone();
    let server = thread::spawn(move || run(server_endpoint).expect("server loop"));

    let mut client = None;
    for _ in 0..100 {
      match transport::connect(&endpoint) {
        Ok(stream) => {
          client = Some(stream);
          break;
        }
        Err(_) => thread::sleep(Duration::from_millis(10)),
      }
    }
    let mut client = client.expect("server listener becomes ready");
    let working_directory = std::env::current_dir().expect("current directory");
    write_frame(
      &mut client,
      &ClientMessage::Hello(ClientHello {
        protocol_version: zmux::protocol::PROTOCOL_VERSION,
        session_name: None,
        create_if_missing: false,
        fail_if_exists: false,
        shell: "/bin/sh".into(),
        environment: vec![("SHELL".to_owned(), "/bin/sh".to_owned())],
        working_directory: working_directory.clone(),
        viewport: Viewport {
          rows: 24,
          columns: 80,
        },
      }),
    )
    .expect("send Client hello");

    assert!(matches!(
      read_frame::<_, ServerMessage>(&mut client).expect("read acceptance"),
      ServerMessage::HelloAccepted { session_name, .. } if session_name == "1"
    ));
    let ServerMessage::InitialState { session, .. } =
      read_frame::<_, ServerMessage>(&mut client).expect("read initial state")
    else {
      panic!("Server should send initial state after accepting Client");
    };
    assert_eq!(session.number, "1");
    assert_eq!(session.workspaces.len(), 1);
    assert_eq!(session.workspaces[0].working_directory, working_directory);
    assert_eq!(session.workspaces[0].tabs.len(), 1);

    write_frame(&mut client, &ClientMessage::Request {
      id: 1,
      request: ClientRequest::Detach,
    })
    .expect("detach Client");
    assert!(matches!(
      read_frame::<_, ServerMessage>(&mut client).expect("read detach reply"),
      ServerMessage::Reply { result: Ok(()), .. }
    ));
    assert_eq!(
      read_frame::<_, ServerMessage>(&mut client).expect("read detach event"),
      ServerMessage::Detached
    );
    drop(client);

    let (mut reattached, reattached_state, _) =
      connect_test_client(&endpoint, Some("1".to_owned()), Viewport {
        rows: 24,
        columns: 80,
      });
    assert_eq!(reattached_state.number, "1");
    assert_eq!(reattached_state.workspaces.len(), 1);

    write_frame(&mut reattached, &ClientMessage::Request {
      id: 2,
      request: ClientRequest::StopServer,
    })
    .expect("request Server shutdown");
    assert_server_stopping(&mut reattached);
    drop(reattached);
    server.join().expect("Server terminates after stop-server");
  }

  // Developer approved: do not remove.
  #[test]
  fn headless_requests_create_list_rename_and_stop_without_client_attachment() {
    let endpoint =
      Endpoint::new(Some(&format!("headless-test-{}", std::process::id()))).expect("test endpoint");
    let server_endpoint = endpoint.clone();
    let server = thread::spawn(move || run(server_endpoint).expect("server loop"));
    let mut client = connect_for_hello(&endpoint);
    let directory = std::env::current_dir().expect("current directory");
    let default_workspace_id = format!(
      "headless:{}",
      directory
        .file_name()
        .expect("current directory has a final component")
        .to_string_lossy()
    );
    write_frame(
      &mut client,
      &ClientMessage::HeadlessHello(HeadlessHello {
        protocol_version: zmux::protocol::PROTOCOL_VERSION,
        working_directory: directory.clone(),
        shell: "/bin/sh".into(),
        environment: vec![("SHELL".to_owned(), "/bin/sh".to_owned())],
      }),
    )
    .expect("send headless hello");

    let request =
      |client: &mut interprocess::local_socket::prelude::LocalSocketStream, id, request| {
        write_frame(client, &ClientMessage::HeadlessRequest { id, request })
          .expect("send headless request");
        match read_frame::<_, ServerMessage>(client).expect("read headless reply") {
          ServerMessage::HeadlessReply {
            id: reply_id,
            result,
          } if reply_id == id => result,
          message => panic!("unexpected headless response: {message:?}"),
        }
      };

    assert_eq!(
      request(&mut client, 1, HeadlessRequest::SessionCreate {
        name: Some("headless".to_owned()),
        workspace_name: Some("project".to_owned()),
      },),
      Ok(HeadlessResult::Accepted)
    );
    assert_eq!(
      request(&mut client, 2, HeadlessRequest::WorkspaceCreate {
        session_id: Some("headless".to_owned()),
        name: None,
        working_directory: directory,
      },),
      Ok(HeadlessResult::Accepted)
    );
    assert!(
      request(&mut client, 3, HeadlessRequest::SessionCreate {
        name: Some("bad:name".to_owned()),
        workspace_name: None,
      })
      .is_err()
    );
    let workspaces = request(&mut client, 4, HeadlessRequest::WorkspaceList {
      session_id: Some("headless".to_owned()),
    })
    .expect("list Workspaces");
    let HeadlessResult::Workspaces(workspaces) = workspaces else {
      panic!("workspace list should return workspace data");
    };
    assert_eq!(
      workspaces
        .iter()
        .map(|workspace| workspace.id.clone())
        .collect::<Vec<_>>(),
      vec!["headless:project".to_owned(), default_workspace_id]
    );

    assert_eq!(
      request(&mut client, 5, HeadlessRequest::TabCreate {
        workspace_id: Some("headless:project".to_owned()),
        name: Some("extra".to_owned()),
      }),
      Ok(HeadlessResult::Accepted)
    );
    let tabs = request(&mut client, 6, HeadlessRequest::TabList {
      workspace_id: Some("headless:project".to_owned()),
    })
    .expect("list Tabs");
    let HeadlessResult::Tabs(tabs) = tabs else {
      panic!("tab list should return tab data");
    };
    assert!(tabs.iter().any(|tab| tab.id == "headless:project:extra"));
    assert_eq!(
      request(&mut client, 7, HeadlessRequest::TabRename {
        id: Some("headless:project:extra".to_owned()),
        name: "renamed-tab".to_owned(),
      }),
      Ok(HeadlessResult::Accepted)
    );
    assert_eq!(
      request(&mut client, 8, HeadlessRequest::TabFocus {
        id: Some("headless:project:renamed-tab".to_owned()),
      }),
      Ok(HeadlessResult::Accepted)
    );
    assert_eq!(
      request(&mut client, 9, HeadlessRequest::TabRemove {
        id: Some("headless:project:renamed-tab".to_owned()),
      }),
      Ok(HeadlessResult::Accepted)
    );

    let (mut attached, _, _) =
      connect_test_client(&endpoint, Some("headless".to_owned()), Viewport {
        rows: 24,
        columns: 80,
      });
    assert_eq!(
      request(&mut client, 10, HeadlessRequest::SessionRename {
        id: Some("headless".to_owned()),
        name: "renamed".to_owned(),
      },),
      Ok(HeadlessResult::Accepted)
    );
    let workspaces = request(&mut client, 11, HeadlessRequest::WorkspaceList {
      session_id: Some("renamed".to_owned()),
    })
    .expect("list Workspaces after Session rename");
    let HeadlessResult::Workspaces(workspaces) = workspaces else {
      panic!("workspace list should return workspace data");
    };
    assert_eq!(workspaces[0].id, "renamed:project");
    assert_eq!(read_state_with_session_name(&mut attached, "renamed").number, "renamed");

    assert_eq!(request(&mut client, 12, HeadlessRequest::StopServer), Ok(HeadlessResult::Accepted));
    drop(attached);
    drop(client);
    server
      .join()
      .expect("Server terminates after headless stop");
  }
}
