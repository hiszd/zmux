use std::io::{
  self,
  Read,
  Write,
};

use serde::{
  Deserialize,
  Serialize,
  de::DeserializeOwned,
};

/// Wire protocol version required by connected Clients.
pub const PROTOCOL_VERSION: u16 = 9;
/// Maximum payload size accepted for one framed IPC message.
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// Terminal dimensions, expressed in character cells.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct Viewport {
  /// Number of rows.
  pub rows: u16,
  /// Number of columns.
  pub columns: u16,
}

/// TUI Client parameters sent when opening a connection.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ClientHello {
  /// Protocol version the Client implements.
  pub protocol_version: u16,
  /// Session to attach to, or `None` to choose one according to the flags.
  pub session_name: Option<String>,
  /// Whether the Server may create the Session when it is missing.
  pub create_if_missing: bool,
  /// Whether the Server should reject an already-existing Session.
  pub fail_if_exists: bool,
  /// Shell executable used for newly created Tabs.
  pub shell: std::path::PathBuf,
  /// Environment variables supplied to newly created shells.
  pub environment: Vec<(String, String)>,
  /// Initial working directory for a newly created Session.
  pub working_directory: std::path::PathBuf,
  /// Initial PTY dimensions requested by the Client.
  pub viewport: Viewport,
}

/// Server-authoritative state for one Workspace.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct WorkspaceSnapshot {
  /// Stable Workspace identifier.
  pub id: u64,
  /// Display name, explicit or derived from the working directory.
  pub name: String,
  /// User-assigned name, if the Workspace has one.
  pub explicit_name: Option<String>,
  /// Directory used when creating Tabs in the Workspace.
  pub working_directory: std::path::PathBuf,
  /// Tabs in Workspace order.
  pub tabs: Vec<TabSnapshot>,
  /// Identifier of the active Tab.
  pub active_tab_id: u64,
}

/// Server-authoritative state for one Tab.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct TabSnapshot {
  /// Stable Tab identifier.
  pub id: u64,
  /// User-visible Tab name.
  pub name: String,
  /// Current terminal title.
  pub title: String,
  /// Process exit code, or `None` while the process is running.
  pub exit_code: Option<u32>,
}

/// Server-authoritative state for one Session.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SessionSnapshot {
  /// Session's user-visible name.
  pub number: String,
  /// Workspaces in Session order.
  pub workspaces: Vec<WorkspaceSnapshot>,
  /// Identifier of the active Workspace.
  pub active_workspace_id: u64,
}

/// Terminal-emulator contents and geometry for one Tab.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct TerminalSnapshot {
  /// Identifier of the Tab represented by this snapshot.
  pub tab_id: u64,
  /// Terminal row count.
  pub rows: u16,
  /// Terminal column count.
  pub columns: u16,
  /// Escape-sequence replay data used to reconstruct the terminal screen.
  pub bytes: Vec<u8>,
  /// Current terminal title.
  pub title: String,
  /// Process exit code, or `None` while the process is running.
  pub exit_code: Option<u32>,
}

/// Top-level message sent by an attached Client.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum ClientMessage {
  Hello(ClientHello),
  HeadlessHello(HeadlessHello),
  Admin(AdminRequest),
  Request { id: u64, request: ClientRequest },
  HeadlessRequest { id: u64, request: HeadlessRequest },
}

/// Connection parameters for a headless Lua client.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct HeadlessHello {
  /// Protocol version the client implements.
  pub protocol_version: u16,
  /// Working directory used when the script creates a Session.
  pub working_directory: std::path::PathBuf,
  /// Shell executable used for newly created Tabs.
  pub shell: std::path::PathBuf,
  /// Environment variables supplied to newly created shells.
  pub environment: Vec<(String, String)>,
}

/// Server operation requested by a headless Lua client.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum HeadlessRequest {
  SessionCreate {
    name: Option<String>,
    workspace_name: Option<String>,
  },
  SessionList,
  SessionRename {
    id: Option<String>,
    name: String,
  },
  SessionRemove {
    id: Option<String>,
  },
  SessionFocus {
    id: Option<String>,
  },
  WorkspaceCreate {
    session_id: Option<String>,
    name: Option<String>,
    working_directory: std::path::PathBuf,
  },
  WorkspaceList {
    session_id: Option<String>,
  },
  WorkspaceRename {
    id: Option<String>,
    name: String,
  },
  WorkspaceSetCwd {
    id: Option<String>,
    working_directory: std::path::PathBuf,
  },
  WorkspaceFocus {
    id: Option<String>,
  },
  WorkspaceRemove {
    id: Option<String>,
  },
  TabCreate {
    workspace_id: Option<String>,
    name: Option<String>,
  },
  TabList {
    workspace_id: Option<String>,
  },
  TabRename {
    id: Option<String>,
    name: String,
  },
  TabFocus {
    id: Option<String>,
  },
  TabRemove {
    id: Option<String>,
  },
  StopServer,
}

/// Successful result returned by a headless operation.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum HeadlessResult {
  Accepted,
  Sessions(Vec<HeadlessSession>),
  Workspaces(Vec<HeadlessWorkspace>),
  Tabs(Vec<HeadlessTab>),
}

/// Summary of a Session returned to a headless client.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct HeadlessSession {
  /// Session selector.
  pub id: String,
  /// User-visible Session name.
  pub name: String,
  /// Number of Workspaces in the Session.
  pub workspaces: usize,
  /// Number of Tabs across the Session.
  pub tabs: usize,
}
/// Workspace details returned to a headless client.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct HeadlessWorkspace {
  /// Qualified Workspace selector.
  pub id: String,
  /// User-visible Workspace name.
  pub name: String,
  /// Directory used when creating Tabs in the Workspace.
  pub working_directory: std::path::PathBuf,
  /// Whether this Workspace is active in its Session.
  pub active: bool,
}
/// Tab details returned to a headless client.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct HeadlessTab {
  /// Qualified Tab selector.
  pub id: String,
  /// User-visible Tab name.
  pub name: String,
  /// Current terminal title.
  pub title: String,
  /// Process exit code, or `None` while the process is running.
  pub exit_code: Option<u32>,
  /// Whether this Tab is active in its Workspace.
  pub active: bool,
}

/// Administrative operation sent to the Server process.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum AdminRequest {
  ListSessions,
  KillSession(String),
  StopServer,
  Status,
  PrepareSession {
    session_name: Option<String>,
    create_if_missing: bool,
    fail_if_exists: bool,
    working_directory: std::path::PathBuf,
    shell: std::path::PathBuf,
    environment: Vec<(String, String)>,
    viewport: Viewport,
  },
}

/// Summary of a Session returned by an administrative query.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SessionSummary {
  /// User-visible Session name.
  pub number: String,
  /// Number of attached TUI Clients.
  pub clients: usize,
  /// Number of Workspaces in the Session.
  pub workspaces: usize,
  /// Number of Tabs across the Session.
  pub tabs: usize,
}

/// Response to an administrative request.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum AdminResponse {
  Sessions(Vec<SessionSummary>),
  Status { running: bool, sessions: usize },
  Success,
  SessionReady { name: String, created: bool },
}

/// Operation requested by an attached TUI Client.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum ClientRequest {
  Input(Vec<u8>),
  ListSessions,
  SwitchSession(String),
  Resize(Viewport),
  NewTab,
  CloseTab,
  SelectTab(usize),
  CreateWorkspace {
    working_directory: std::path::PathBuf,
    name: Option<String>,
  },
  EditWorkspace {
    id: u64,
    working_directory: std::path::PathBuf,
    name: Option<String>,
  },
  SelectWorkspace(u64),
  CloseWorkspace,
  Detach,
  KillSession,
  StopServer,
}

/// Message sent by the Server to a connected Client.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum ServerMessage {
  HelloAccepted {
    client_id: u64,
    session_name: String,
  },
  HelloRejected {
    reason: String,
  },
  AdminResult {
    result: Result<AdminResponse, String>,
  },
  HeadlessReply {
    id: u64,
    result: Result<HeadlessResult, String>,
  },
  InitialState {
    session: SessionSnapshot,
    terminals: Vec<TerminalSnapshot>,
  },
  Reply {
    id: u64,
    result: Result<(), String>,
  },
  SessionList {
    sessions: Vec<SessionSummary>,
  },
  StateChanged {
    session: SessionSnapshot,
    /// Terminal snapshots to update; empty for selection-only state changes.
    terminals: Vec<TerminalSnapshot>,
  },
  TerminalOutput {
    tab_id: u64,
    bytes: Vec<u8>,
  },
  ClipboardStore {
    tab_id: u64,
    text: String,
  },
  TerminalChanged(TerminalSnapshot),
  Detached,
  SessionClosed,
  ServerStopping,
}

/// Serializes and writes one length-prefixed IPC message.
///
/// The encoded payload is limited to [`MAX_FRAME_BYTES`].
///
/// # Errors
///
/// Returns an error if serialization fails, the payload exceeds the limit, or writing fails.
pub fn write_frame<W, T>(writer: &mut W, message: &T) -> io::Result<()>
where
  W: Write,
  T: Serialize,
{
  let payload =
    bincode::serde::encode_to_vec(message, bincode::config::legacy()).map_err(invalid_data)?;
  if payload.len() > MAX_FRAME_BYTES {
    return Err(io::Error::new(
      io::ErrorKind::InvalidData,
      format!("IPC frame exceeds {MAX_FRAME_BYTES} bytes"),
    ));
  }
  let length = u32::try_from(payload.len()).map_err(invalid_data)?;
  writer.write_all(&length.to_be_bytes())?;
  writer.write_all(&payload)?;
  writer.flush()
}

/// Reads and deserializes one length-prefixed IPC message.
///
/// Rejects payloads larger than [`MAX_FRAME_BYTES`] and trailing bytes after the decoded value.
///
/// # Errors
///
/// Returns an error if reading, validation, or deserialization fails.
pub fn read_frame<R, T>(reader: &mut R) -> io::Result<T>
where
  R: Read,
  T: DeserializeOwned,
{
  let mut length_bytes = [0_u8; 4];
  reader.read_exact(&mut length_bytes)?;
  let length = u32::from_be_bytes(length_bytes) as usize;
  if length > MAX_FRAME_BYTES {
    return Err(io::Error::new(
      io::ErrorKind::InvalidData,
      format!("IPC frame exceeds {MAX_FRAME_BYTES} bytes"),
    ));
  }
  let mut payload = vec![0; length];
  reader.read_exact(&mut payload)?;
  let (message, consumed): (T, usize) =
    bincode::serde::decode_from_slice(&payload, bincode::config::legacy()).map_err(invalid_data)?;
  if consumed != payload.len() {
    return Err(io::Error::new(io::ErrorKind::InvalidData, "trailing bytes after IPC payload"));
  }
  Ok(message)
}

fn invalid_data(error: impl std::fmt::Display) -> io::Error {
  io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
  use std::io::Cursor;

  use super::{
    ClientHello,
    ClientMessage,
    ClientRequest,
    PROTOCOL_VERSION,
    Viewport,
    read_frame,
    write_frame,
  };

  // Developer approved: do not remove.
  #[test]
  fn framed_messages_round_trip_with_binary_terminal_input() {
    let message = ClientMessage::Request {
      id: 19,
      request: ClientRequest::Input(vec![0, 0xff, b'\n']),
    };
    let mut frame = Cursor::new(Vec::new());

    write_frame(&mut frame, &message).expect("write message frame");
    frame.set_position(0);
    let decoded: ClientMessage = read_frame(&mut frame).expect("read message frame");

    assert_eq!(decoded, message);
  }

  // Developer approved: do not remove.
  #[test]
  fn legacy_bincode_frames_remain_readable_and_writable() {
    let message = ClientMessage::Hello(ClientHello {
      protocol_version: PROTOCOL_VERSION,
      session_name: Some("project".to_owned()),
      create_if_missing: false,
      fail_if_exists: false,
      shell: "/bin/sh".into(),
      environment: vec![("SHELL".to_owned(), "/bin/sh".to_owned())],
      working_directory: "/work/project".into(),
      viewport: Viewport {
        rows: 40,
        columns: 120,
      },
    });
    let legacy_payload = bincode_v1::serialize(&message).expect("serialize legacy payload");
    let legacy_length = u32::try_from(legacy_payload.len()).expect("frame length fits u32");

    let mut encoded = Cursor::new(Vec::new());
    write_frame(&mut encoded, &message).expect("write current frame");
    let encoded = encoded.into_inner();
    assert_eq!(&encoded[..4], &legacy_length.to_be_bytes());
    assert_eq!(&encoded[4..], legacy_payload);

    let mut legacy_frame = legacy_length.to_be_bytes().to_vec();
    legacy_frame.extend_from_slice(&legacy_payload);
    let decoded: ClientMessage =
      read_frame(&mut Cursor::new(legacy_frame)).expect("read legacy frame");
    assert_eq!(decoded, message);
  }

  // Developer approved: do not remove.
  #[test]
  fn frame_rejects_trailing_payload_bytes() {
    let message = ClientMessage::Request {
      id: 19,
      request: ClientRequest::Input(vec![0, 0xff, b'\n']),
    };
    let mut payload = bincode_v1::serialize(&message).expect("serialize payload");
    payload.push(0);
    let length = u32::try_from(payload.len()).expect("frame length fits u32");
    let mut frame = length.to_be_bytes().to_vec();
    frame.extend_from_slice(&payload);

    let error = read_frame::<_, ClientMessage>(&mut Cursor::new(frame))
      .expect_err("trailing payload bytes should be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
  }

  // Developer approved: do not remove.
  #[test]
  fn hello_frame_preserves_target_and_client_viewport() {
    let message = ClientMessage::Hello(ClientHello {
      protocol_version: 1,
      session_name: Some("8".to_owned()),
      create_if_missing: false,
      fail_if_exists: false,
      shell: "/bin/sh".into(),
      environment: vec![("SHELL".to_owned(), "/bin/sh".to_owned())],
      working_directory: "/work/project".into(),
      viewport: Viewport {
        rows: 40,
        columns: 120,
      },
    });
    let mut frame = Cursor::new(Vec::new());

    write_frame(&mut frame, &message).expect("write hello frame");
    frame.set_position(0);
    let decoded: ClientMessage = read_frame(&mut frame).expect("read hello frame");

    assert_eq!(decoded, message);
  }
}
