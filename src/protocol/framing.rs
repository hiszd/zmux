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

pub const PROTOCOL_VERSION: u16 = 7;
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct Viewport {
  pub rows: u16,
  pub columns: u16,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ClientHello {
  pub protocol_version: u16,
  pub session_name: Option<String>,
  pub create_if_missing: bool,
  pub fail_if_exists: bool,
  pub shell: std::path::PathBuf,
  pub environment: Vec<(String, String)>,
  pub working_directory: std::path::PathBuf,
  pub viewport: Viewport,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct WorkspaceSnapshot {
  pub id: u64,
  pub name: String,
  pub explicit_name: Option<String>,
  pub working_directory: std::path::PathBuf,
  pub tabs: Vec<TabSnapshot>,
  pub active_tab_id: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct TabSnapshot {
  pub id: u64,
  pub name: String,
  pub title: String,
  pub exit_code: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SessionSnapshot {
  pub number: String,
  pub workspaces: Vec<WorkspaceSnapshot>,
  pub active_workspace_id: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct TerminalSnapshot {
  pub tab_id: u64,
  pub rows: u16,
  pub columns: u16,
  pub bytes: Vec<u8>,
  pub title: String,
  pub exit_code: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum ClientMessage {
  Hello(ClientHello),
  HeadlessHello(HeadlessHello),
  Admin(AdminRequest),
  Request { id: u64, request: ClientRequest },
  HeadlessRequest { id: u64, request: HeadlessRequest },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct HeadlessHello {
  pub protocol_version: u16,
  pub working_directory: std::path::PathBuf,
  pub shell: std::path::PathBuf,
  pub environment: Vec<(String, String)>,
}

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

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum HeadlessResult {
  Accepted,
  Sessions(Vec<HeadlessSession>),
  Workspaces(Vec<HeadlessWorkspace>),
  Tabs(Vec<HeadlessTab>),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct HeadlessSession {
  pub id: String,
  pub name: String,
  pub workspaces: usize,
  pub tabs: usize,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct HeadlessWorkspace {
  pub id: String,
  pub name: String,
  pub working_directory: std::path::PathBuf,
  pub active: bool,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct HeadlessTab {
  pub id: String,
  pub name: String,
  pub title: String,
  pub exit_code: Option<u32>,
  pub active: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum AdminRequest {
  ListSessions,
  KillSession(String),
  StopServer,
  Status,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SessionSummary {
  pub number: String,
  pub clients: usize,
  pub workspaces: usize,
  pub tabs: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum AdminResponse {
  Sessions(Vec<SessionSummary>),
  Status { running: bool, sessions: usize },
  Success,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum ClientRequest {
  Input(Vec<u8>),
  ListSessions,
  SwitchSession(String),
  Resize(Viewport),
  NewTab,
  CloseTab,
  NextTab,
  PreviousTab,
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
