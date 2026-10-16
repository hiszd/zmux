use std::{
  cell::RefCell,
  collections::VecDeque,
  io,
  path::PathBuf,
  rc::Rc,
};

use mlua::{
  Function,
  Lua,
  Table,
  Value,
  Variadic,
};
use zmux::{
  protocol::{
    ClientMessage,
    HeadlessHello,
    HeadlessRequest,
    HeadlessResult,
    HeadlessSession,
    HeadlessTab,
    HeadlessWorkspace,
    PROTOCOL_VERSION,
    ServerMessage,
    read_frame,
    write_frame,
  },
  transport::{
    self,
    Endpoint,
  },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptInput {
  File(PathBuf),
  Source(String),
}

struct Pending {
  request: HeadlessRequest,
  callback: Option<Function>,
}

type CommandParser = fn(Variadic<Value>) -> mlua::Result<(HeadlessRequest, Option<Function>)>;

pub fn run(
  endpoint: Endpoint,
  script: ScriptInput,
  arguments: &[String],
  working_directory: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
  let (source, chunk_name) = match script {
    ScriptInput::File(path) => {
      (std::fs::read_to_string(&path)?, path.to_string_lossy().into_owned())
    }
    ScriptInput::Source(source) => (source, "=(command line)".to_owned()),
  };
  let lua = Lua::new();
  let queue = Rc::new(RefCell::new(VecDeque::<Pending>::new()));
  let cmd = lua.create_table()?;
  let session = lua.create_table()?;
  let workspace = lua.create_table()?;
  let tab = lua.create_table()?;
  let server = lua.create_table()?;
  cmd.set("session", session.clone())?;
  cmd.set("workspace", workspace.clone())?;
  cmd.set("tab", tab.clone())?;
  cmd.set("server", server.clone())?;
  lua.globals().set("cmd", cmd)?;
  let args = lua.create_table_with_capacity(arguments.len(), 0)?;
  for (index, argument) in arguments.iter().enumerate() {
    args.set(index + 1, argument.as_str())?;
  }
  lua.globals().set("args", args)?;
  let g = lua.create_table()?;
  g.set("session", Value::Nil)?;
  g.set("active_workspace", Value::Nil)?;
  g.set("active_tab", Value::Nil)?;
  g.set("last_error", Value::Nil)?;
  g.set("mode", "headless")?;
  lua.globals().set("g", g.clone())?;
  lua.globals().set("global", g.clone())?;

  register_commands(&lua, &session, &workspace, &tab, &server, &queue)?;
  let mut failed = false;
  if let Err(error) = lua.load(source).set_name(chunk_name).exec() {
    eprintln!("zmux run: {error}");
    g.set("last_error", error.to_string())?;
    failed = true;
  }

  let mut stream = transport::connect(&endpoint)?;
  write_frame(
    &mut stream,
    &ClientMessage::HeadlessHello(HeadlessHello {
      protocol_version: PROTOCOL_VERSION,
      working_directory,
      shell: std::env::var_os("SHELL")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/bin/sh")),
      environment: std::env::vars().collect(),
    }),
  )?;

  let mut next_id = 1_u64;
  let mut connection_open = true;
  loop {
    let pending = queue.borrow_mut().pop_front();
    let Some(mut pending) = pending else {
      break;
    };
    let result = if connection_open {
      let id = next_id;
      next_id = next_id
        .checked_add(1)
        .ok_or("headless request identifier exhausted")?;
      match write_frame(&mut stream, &ClientMessage::HeadlessRequest {
        id,
        request: pending.request,
      }) {
        Ok(()) => match read_frame::<_, ServerMessage>(&mut stream) {
          Ok(ServerMessage::HeadlessReply {
            id: reply_id,
            result,
          }) if reply_id == id => result,
          Ok(other) => {
            connection_open = false;
            Err(format!("unexpected server response: {other:?}"))
          }
          Err(error) => {
            connection_open = false;
            Err(format!("server connection failed before accepting command: {error}"))
          }
        },
        Err(error) => {
          connection_open = false;
          Err(format!("server connection failed before accepting command: {error}"))
        }
      }
    } else {
      Err("server connection closed before command was accepted".to_owned())
    };
    match result {
      Ok(value) => {
        g.set("last_error", Value::Nil)?;
        if let Some(callback) = pending.callback.take() {
          let lua_value = result_to_lua(&lua, value)?;
          if let Err(error) = callback.call::<()>(lua_value) {
            let message = error.to_string();
            eprintln!("zmux run: {message}");
            g.set("last_error", message)?;
            failed = true;
          }
        }
      }
      Err(error) => {
        eprintln!("zmux run: {error}");
        g.set("last_error", error)?;
        failed = true;
      }
    }
  }
  if failed {
    Err(io::Error::other("headless script failed").into())
  } else {
    Ok(())
  }
}

fn register_commands(
  lua: &Lua,
  session: &Table,
  workspace: &Table,
  tab: &Table,
  server: &Table,
  queue: &Rc<RefCell<VecDeque<Pending>>>,
) -> mlua::Result<()> {
  let add = |namespace: &Table,
             name: &str,
             queue: Rc<RefCell<VecDeque<Pending>>>,
             parser: CommandParser|
   -> mlua::Result<()> {
    namespace.set(
      name,
      lua.create_function(move |_, values: Variadic<Value>| {
        let (request, callback) = parser(values)?;
        queue.borrow_mut().push_back(Pending { request, callback });
        Ok(())
      })?,
    )
  };
  add(session, "create", queue.clone(), parse_session_create)?;
  add(session, "list", queue.clone(), parse_session_list)?;
  add(session, "rename", queue.clone(), parse_session_rename)?;
  add(session, "remove", queue.clone(), parse_session_remove)?;
  add(session, "focus", queue.clone(), parse_session_focus)?;
  add(workspace, "create", queue.clone(), parse_workspace_create)?;
  add(workspace, "list", queue.clone(), parse_workspace_list)?;
  add(workspace, "rename", queue.clone(), parse_workspace_rename)?;
  add(workspace, "setcwd", queue.clone(), parse_workspace_setcwd)?;
  add(workspace, "focus", queue.clone(), parse_workspace_focus)?;
  add(workspace, "remove", queue.clone(), parse_workspace_remove)?;
  add(tab, "create", queue.clone(), parse_tab_create)?;
  add(tab, "list", queue.clone(), parse_tab_list)?;
  add(tab, "rename", queue.clone(), parse_tab_rename)?;
  add(tab, "focus", queue.clone(), parse_tab_focus)?;
  add(tab, "remove", queue.clone(), parse_tab_remove)?;
  add(server, "stop", queue.clone(), |_| Ok((HeadlessRequest::StopServer, None)))?;
  Ok(())
}

fn string(value: Option<&Value>, label: &str) -> mlua::Result<Option<String>> {
  match value {
    None | Some(Value::Nil) => Ok(None),
    Some(Value::String(value)) => Ok(Some(value.to_str()?.to_owned())),
    Some(_) => Err(mlua::Error::external(format!("{label} must be a string"))),
  }
}
fn table(value: Option<&Value>) -> mlua::Result<Option<Table>> {
  match value {
    None | Some(Value::Nil) => Ok(None),
    Some(Value::Table(value)) => Ok(Some(value.clone())),
    Some(_) => Err(mlua::Error::external("options must be a table")),
  }
}
fn callback(value: Option<&Value>) -> mlua::Result<Option<Function>> {
  match value {
    Some(Value::Function(value)) => Ok(Some(value.clone())),
    None | Some(Value::Nil) => Ok(None),
    Some(_) => Err(mlua::Error::external("callback must be a function")),
  }
}
fn option_name(options: Option<Table>) -> mlua::Result<Option<String>> {
  options.map(|value| value.get("name")).transpose()
}
fn parse_session_create(
  values: Variadic<Value>,
) -> mlua::Result<(HeadlessRequest, Option<Function>)> {
  let name = string(values.first(), "Session name")?;
  let options = table(values.get(1))?;
  let workspace_name = options
    .map(|v| v.get("workspace_name"))
    .transpose()?
    .flatten();
  Ok((
    HeadlessRequest::SessionCreate {
      name,
      workspace_name,
    },
    None,
  ))
}
fn parse_session_list(
  values: Variadic<Value>,
) -> mlua::Result<(HeadlessRequest, Option<Function>)> {
  Ok((HeadlessRequest::SessionList, callback(values.first())?))
}
fn parse_session_rename(
  values: Variadic<Value>,
) -> mlua::Result<(HeadlessRequest, Option<Function>)> {
  Ok((
    HeadlessRequest::SessionRename {
      id: string(values.first(), "Session ID")?,
      name: string(values.get(1), "new name")?
        .ok_or_else(|| mlua::Error::external("new name is required"))?,
    },
    None,
  ))
}
fn parse_session_remove(
  values: Variadic<Value>,
) -> mlua::Result<(HeadlessRequest, Option<Function>)> {
  Ok((
    HeadlessRequest::SessionRemove {
      id: string(values.first(), "Session ID")?,
    },
    None,
  ))
}
fn parse_session_focus(
  values: Variadic<Value>,
) -> mlua::Result<(HeadlessRequest, Option<Function>)> {
  Ok((
    HeadlessRequest::SessionFocus {
      id: string(values.first(), "Session ID")?,
    },
    None,
  ))
}
fn parse_workspace_create(
  values: Variadic<Value>,
) -> mlua::Result<(HeadlessRequest, Option<Function>)> {
  let parent = string(values.first(), "Session ID")?;
  let opts =
    table(values.get(1))?.ok_or_else(|| mlua::Error::external("workspace options are required"))?;
  let directory: String = opts.get("working_directory")?;
  Ok((
    HeadlessRequest::WorkspaceCreate {
      session_id: parent,
      name: option_name(Some(opts))?,
      working_directory: directory.into(),
    },
    None,
  ))
}
fn parse_workspace_list(
  values: Variadic<Value>,
) -> mlua::Result<(HeadlessRequest, Option<Function>)> {
  let (session_id, callback_value) = if values.get(1).is_some() {
    (string(values.first(), "Session ID")?, values.get(1))
  } else {
    (None, values.first())
  };
  Ok((HeadlessRequest::WorkspaceList { session_id }, callback(callback_value)?))
}
fn parse_workspace_rename(
  values: Variadic<Value>,
) -> mlua::Result<(HeadlessRequest, Option<Function>)> {
  Ok((
    HeadlessRequest::WorkspaceRename {
      id: string(values.first(), "Workspace ID")?,
      name: string(values.get(1), "new name")?
        .ok_or_else(|| mlua::Error::external("new name is required"))?,
    },
    None,
  ))
}
fn parse_workspace_setcwd(
  values: Variadic<Value>,
) -> mlua::Result<(HeadlessRequest, Option<Function>)> {
  Ok((
    HeadlessRequest::WorkspaceSetCwd {
      id: string(values.first(), "Workspace ID")?,
      working_directory: string(values.get(1), "working directory")?
        .ok_or_else(|| mlua::Error::external("working directory is required"))?
        .into(),
    },
    None,
  ))
}
fn parse_workspace_focus(
  values: Variadic<Value>,
) -> mlua::Result<(HeadlessRequest, Option<Function>)> {
  Ok((
    HeadlessRequest::WorkspaceFocus {
      id: string(values.first(), "Workspace ID")?,
    },
    None,
  ))
}
fn parse_workspace_remove(
  values: Variadic<Value>,
) -> mlua::Result<(HeadlessRequest, Option<Function>)> {
  Ok((
    HeadlessRequest::WorkspaceRemove {
      id: string(values.first(), "Workspace ID")?,
    },
    None,
  ))
}
fn parse_tab_create(values: Variadic<Value>) -> mlua::Result<(HeadlessRequest, Option<Function>)> {
  let opts = table(values.get(1))?;
  Ok((
    HeadlessRequest::TabCreate {
      workspace_id: string(values.first(), "Workspace ID")?,
      name: option_name(opts)?,
    },
    None,
  ))
}
fn parse_tab_list(values: Variadic<Value>) -> mlua::Result<(HeadlessRequest, Option<Function>)> {
  let (workspace_id, callback_value) = if values.get(1).is_some() {
    (string(values.first(), "Workspace ID")?, values.get(1))
  } else {
    (None, values.first())
  };
  Ok((HeadlessRequest::TabList { workspace_id }, callback(callback_value)?))
}
fn parse_tab_rename(values: Variadic<Value>) -> mlua::Result<(HeadlessRequest, Option<Function>)> {
  Ok((
    HeadlessRequest::TabRename {
      id: string(values.first(), "Tab ID")?,
      name: string(values.get(1), "new name")?
        .ok_or_else(|| mlua::Error::external("new name is required"))?,
    },
    None,
  ))
}
fn parse_tab_focus(values: Variadic<Value>) -> mlua::Result<(HeadlessRequest, Option<Function>)> {
  Ok((
    HeadlessRequest::TabFocus {
      id: string(values.first(), "Tab ID")?,
    },
    None,
  ))
}
fn parse_tab_remove(values: Variadic<Value>) -> mlua::Result<(HeadlessRequest, Option<Function>)> {
  Ok((
    HeadlessRequest::TabRemove {
      id: string(values.first(), "Tab ID")?,
    },
    None,
  ))
}

fn result_to_lua(lua: &Lua, result: HeadlessResult) -> mlua::Result<Table> {
  let values = lua.create_table()?;
  match result {
    HeadlessResult::Sessions(items) => {
      for (i, item) in items.into_iter().enumerate() {
        values.set(i + 1, session_to_lua(lua, item)?)?
      }
    }
    HeadlessResult::Workspaces(items) => {
      for (i, item) in items.into_iter().enumerate() {
        values.set(i + 1, workspace_to_lua(lua, item)?)?
      }
    }
    HeadlessResult::Tabs(items) => {
      for (i, item) in items.into_iter().enumerate() {
        values.set(i + 1, tab_to_lua(lua, item)?)?
      }
    }
    HeadlessResult::Accepted => {}
  }
  Ok(values)
}
fn session_to_lua(lua: &Lua, item: HeadlessSession) -> mlua::Result<Table> {
  let t = lua.create_table()?;
  t.set("id", item.id)?;
  t.set("name", item.name)?;
  t.set("workspace_count", item.workspaces)?;
  t.set("tab_count", item.tabs)?;
  Ok(t)
}
fn workspace_to_lua(lua: &Lua, item: HeadlessWorkspace) -> mlua::Result<Table> {
  let t = lua.create_table()?;
  t.set("id", item.id)?;
  t.set("name", item.name)?;
  t.set("working_directory", item.working_directory.to_string_lossy().as_ref())?;
  t.set("active", item.active)?;
  Ok(t)
}
fn tab_to_lua(lua: &Lua, item: HeadlessTab) -> mlua::Result<Table> {
  let t = lua.create_table()?;
  t.set("id", item.id)?;
  t.set("name", item.name)?;
  t.set("title", item.title)?;
  t.set("exit_code", item.exit_code)?;
  t.set("active", item.active)?;
  Ok(t)
}
