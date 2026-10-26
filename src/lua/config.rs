use std::{
  cell::{
    Cell,
    RefCell,
  },
  collections::{
    HashMap,
    VecDeque,
  },
  path::PathBuf,
  rc::Rc,
  sync::mpsc::{
    Receiver,
    Sender,
  },
  time::Duration,
};

use mlua::{
  ErrorContext,
  Function,
  Lua,
  Result as LuaResult,
  Table,
  UserData,
  Value,
};
use ratatui::style::{
  Color,
  Modifier,
  Style,
};
use zmux::protocol::SessionSnapshot;

use crate::{
  input::{
    KeyMap,
    key_notation::{
      KeyStroke,
      normalize_prefix_key,
      parse_key_notation,
    },
  },
  ui::{
    CursorShape,
    DrawerBorder,
    DrawerSide,
    LayoutAxis,
    ToastPosition,
    UiChild,
    UiElementOptions,
    UiElementProperty,
    UiElementType,
    UiElementValue,
    UiSize,
    UiTree,
  },
};

pub const CONFIG_API_VERSION: u32 = 1;
pub const DEFAULT_CONFIG_SOURCE: &str = include_str!("../../defaults/zmux.lua");

pub struct AppConfig {
  pub config_api_version: Option<u32>,
  pub keymap: KeyMap,
  pub cursor_shape: CursorShape,
  pub default_shell: Option<PathBuf>,
  pub prefix_key: KeyStroke,
  pub sequence_timeout: Duration,
  pub lua: LuaConfig,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LaunchState {
  pub command: String,
  pub session_name: Option<String>,
  pub create_if_missing: bool,
  pub fail_if_exists: bool,
  pub require_session: bool,
  pub working_directory: PathBuf,
}

impl Default for AppConfig {
  fn default() -> Self {
    load_config(DEFAULT_CONFIG_SOURCE).expect("built-in Lua defaults are valid")
  }
}

#[derive(Clone, Debug)]
pub struct TabSnapshot {
  pub id: u64,
  pub name: String,
  pub index: usize,
  pub title: String,
  pub selected: bool,
  pub exit_code: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct AppSnapshot {
  pub session: Option<SessionSnapshot>,
  pub last_error: Option<String>,
  pub tabs: Vec<TabSnapshot>,
  pub active_tab: Option<usize>,
  pub leader_mode: bool,
  pub terminal_rows: u16,
  pub terminal_columns: u16,
}

#[derive(Clone, Debug, Default)]
pub struct AppEvent {
  pub name: String,
  pub session_number: Option<String>,
  pub workspace_id: Option<u64>,
  pub workspace_index: Option<usize>,
  pub previous_workspace_id: Option<u64>,
  pub tab_id: Option<u64>,
  pub index: Option<usize>,
  pub previous_index: Option<usize>,
  pub title: Option<String>,
  pub exit_code: Option<u32>,
  pub rows: Option<u16>,
  pub columns: Option<u16>,
  pub active: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeCommand {
  OpenPicker(u64),
  OpenSessionPicker,
  Redraw,
  Detach,
  KillSession,
  StopServer,
  CreateWorkspace,
  EditWorkspace,
  NextWorkspace,
  PreviousWorkspace,
  SelectWorkspace(u64),
  CloseWorkspace,
  NewTab,
  CloseTab,
  NextTab,
  PreviousTab,
  SelectTab(usize),
}

#[derive(Clone, Copy, Debug)]
pub struct AnchorOptions {
  pub height: u16,
  pub visible: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum WhichKeyHeight {
  Percent(u16),
  Rows(u16),
}

impl WhichKeyHeight {
  pub fn rows(self, screen_height: u16) -> u16 {
    let maximum = (u32::from(screen_height) * 60 / 100) as u16;
    match self {
      Self::Percent(percent) => {
        ((u32::from(screen_height) * u32::from(percent)) / 100).min(u32::from(maximum)) as u16
      }
      Self::Rows(rows) => rows.min(maximum),
    }
  }
}

#[derive(Clone, Copy, Debug)]
pub struct WhichKeyOptions {
  pub max_columns: usize,
  pub max_height: WhichKeyHeight,
}

impl Default for WhichKeyOptions {
  fn default() -> Self {
    Self {
      max_columns: 7,
      max_height: WhichKeyHeight::Percent(55),
    }
  }
}

#[derive(Clone, Copy, Debug)]
pub struct PtyOptions {
  pub border: DrawerBorder,
  pub border_style: Style,
}

#[derive(Clone, Copy, Debug)]
pub struct UiOptions {
  pub top: AnchorOptions,
  pub bottom: AnchorOptions,
  pub which: WhichKeyOptions,
  pub pty: PtyOptions,
}

#[derive(Clone, Debug)]
pub struct BarSegment {
  pub text: String,
  pub role: Option<String>,
  pub style: Option<Style>,
}

pub struct LuaCallback {
  function: Function,
  pub interrupt: bool,
}

#[derive(Clone)]
struct RegisteredCallback {
  function: Function,
  interrupt: bool,
}

struct PendingPicker {
  labels: Vec<String>,
  values: Vec<Value>,
  callback: Function,
}

#[derive(Clone, Copy)]
enum UiHandleKind {
  Node,
  Element,
}

#[derive(Clone, Copy)]
struct UiHandleRelease {
  kind: UiHandleKind,
  id: u64,
}

struct UiHandleLease {
  release: UiHandleRelease,
  sender: Sender<UiHandleRelease>,
}

impl UserData for UiHandleLease {
}

impl Drop for UiHandleLease {
  fn drop(&mut self) { let _ = self.sender.send(self.release); }
}

#[derive(Clone)]
struct UiApiContext {
  tree: Rc<RefCell<UiTree>>,
  handles: Table,
  leases: Table,
  release_sender: Sender<UiHandleRelease>,
  commands: Rc<RefCell<VecDeque<RuntimeCommand>>>,
  config_loading: Rc<Cell<bool>>,
  in_callback: Rc<Cell<bool>>,
}

pub struct LuaConfig {
  lua: Lua,
  options: Table,
  globals: Table,
  snapshot: RefCell<AppSnapshot>,
  handlers: Rc<RefCell<HashMap<String, Vec<RegisteredCallback>>>>,
  commands: Rc<RefCell<VecDeque<RuntimeCommand>>>,
  in_event_callback: Rc<Cell<bool>>,
  pending_pickers: Rc<RefCell<HashMap<u64, PendingPicker>>>,
  ui_tree: Rc<RefCell<UiTree>>,
  ui_handles: Table,
  _ui_leases: Table,
  ui_releases: RefCell<Receiver<UiHandleRelease>>,
  launch: Table,
}

impl LuaConfig {
  pub fn sync_state(&self, snapshot: &AppSnapshot) -> LuaResult<()> {
    *self.snapshot.borrow_mut() = snapshot.clone();
    // `Table::clear` in mlua 0.11.6 leaves a stack value behind on LuaJIT.
    let keys = self
      .globals
      .clone()
      .pairs::<Value, Value>()
      .map(|entry| entry.map(|(key, _)| key))
      .collect::<LuaResult<Vec<_>>>()?;
    for key in keys {
      self.globals.set(key, Value::Nil)?;
    }
    let tabs = self
      .lua
      .create_table_with_capacity(snapshot.tabs.len(), 0)?;
    let mut tab_values = Vec::with_capacity(snapshot.tabs.len());
    for tab in &snapshot.tabs {
      let value = self.lua.create_table()?;
      value.set("id", tab.id)?;
      value.set("name", tab.name.as_str())?;
      value.set("index", tab.index)?;
      value.set("title", tab.title.as_str())?;
      value.set("selected", tab.selected)?;
      value.set("exit_code", tab.exit_code)?;
      value.set("running", tab.exit_code.is_none())?;
      tabs.set(tab.index, value.clone())?;
      tab_values.push(value);
    }
    let active_tab = snapshot
      .active_tab
      .and_then(|active| tab_values.get(active).cloned());
    let terminal = self.lua.create_table()?;
    terminal.set("rows", snapshot.terminal_rows)?;
    terminal.set("columns", snapshot.terminal_columns)?;
    self.globals.set("tabs", tabs)?;
    self.globals.set("active_tab", active_tab)?;
    let session_value = if let Some(session) = &snapshot.session {
      let workspaces = self
        .lua
        .create_table_with_capacity(session.workspaces.len(), 0)?;
      let mut workspace_values = Vec::with_capacity(session.workspaces.len());
      for (index, workspace) in session.workspaces.iter().enumerate() {
        let value = self.lua.create_table()?;
        value.set("id", workspace.id)?;
        value.set("index", index + 1)?;
        value.set("name", workspace.name.as_str())?;
        value.set("explicit_name", workspace.explicit_name.as_deref())?;
        value.set("working_directory", workspace.working_directory.to_string_lossy().as_ref())?;
        value.set("selected", workspace.id == session.active_workspace_id)?;
        let workspace_tabs = self
          .lua
          .create_table_with_capacity(workspace.tabs.len(), 0)?;
        let mut tab_values = Vec::with_capacity(workspace.tabs.len());
        for (tab_index, tab) in workspace.tabs.iter().enumerate() {
          let tab_value = self.lua.create_table()?;
          tab_value.set("id", tab.id)?;
          tab_value.set("name", tab.name.as_str())?;
          tab_value.set("index", tab_index + 1)?;
          tab_value.set("title", tab.title.as_str())?;
          tab_value.set("selected", tab.id == workspace.active_tab_id)?;
          tab_value.set("exit_code", tab.exit_code)?;
          tab_value.set("running", tab.exit_code.is_none())?;
          workspace_tabs.set(tab_index + 1, tab_value.clone())?;
          tab_values.push(tab_value);
        }
        let active_workspace_tab = workspace
          .tabs
          .iter()
          .position(|tab| tab.id == workspace.active_tab_id)
          .and_then(|active| tab_values.get(active).cloned());
        value.set("tabs", workspace_tabs)?;
        value.set("active_tab", active_workspace_tab)?;
        workspaces.set(index + 1, value.clone())?;
        workspace_values.push(value);
      }
      let active_workspace = session
        .workspaces
        .iter()
        .position(|workspace| workspace.id == session.active_workspace_id)
        .and_then(|active| workspace_values.get(active).cloned());
      let session_value = self.lua.create_table()?;
      session_value.set("number", session.number.clone())?;
      session_value.set("active_workspace_id", session.active_workspace_id)?;
      session_value.set("workspaces", workspaces.clone())?;
      session_value.set("active_workspace", active_workspace.clone())?;
      self.globals.set("workspaces", workspaces)?;
      self.globals.set("active_workspace", active_workspace)?;
      mlua::Value::Table(session_value)
    } else {
      self.globals.set("workspaces", self.lua.create_table()?)?;
      self.globals.set("active_workspace", mlua::Value::Nil)?;
      mlua::Value::Nil
    };
    self.globals.set("mode", "tui")?;
    self.globals.set("session", session_value)?;
    self
      .globals
      .set("last_error", snapshot.last_error.as_deref())?;
    self.globals.set("leader_mode", snapshot.leader_mode)?;
    self.globals.set("terminal", terminal)?;
    Ok(())
  }

  pub fn event_handlers(&self, name: &str) -> Vec<LuaCallback> {
    self
      .handlers
      .borrow()
      .get(name)
      .cloned()
      .unwrap_or_default()
      .into_iter()
      .map(|handler| LuaCallback {
        function: handler.function,
        interrupt: handler.interrupt,
      })
      .collect()
  }

  pub fn set_launch_session(&self, session_name: Option<&str>) -> LuaResult<()> {
    self.launch.set("session", session_name)?;
    Ok(())
  }

  pub fn render_ui_elements(
    &self,
    area: ratatui::layout::Rect,
    buffer: &mut ratatui::buffer::Buffer,
  ) {
    self.reap_ui_releases();
    self.ui_tree.borrow_mut().render(area, buffer);
  }

  pub fn focused_ui(&self) -> Option<u64> {
    self.reap_ui_releases();
    self.ui_tree.borrow().focused_element()
  }

  pub fn focused_ui_captures_input(&self) -> bool {
    self.reap_ui_releases();
    self.ui_tree.borrow().focused_element_captures_input()
  }

  pub fn expire_ui_toasts(&self, now: std::time::Instant) -> bool {
    self.reap_ui_releases();
    self.ui_tree.borrow_mut().expire_toasts(now)
  }

  pub fn focused_ui_binding(&self, key: KeyStroke) -> Option<(u64, Function, Table)> {
    self.reap_ui_releases();
    let id = self.ui_tree.borrow().focused_element()?;
    let handle = self.ui_handles.get::<Option<Table>>(id).ok().flatten()?;
    let keys = handle.get::<Table>("__zmux_ui_keys").ok()?;
    let key_name = crate::input::key_notation::key_stroke_to_string(&key);
    let callback = keys.get::<Option<Function>>(key_name).ok().flatten()?;
    Some((id, callback, handle))
  }

  pub fn blur_focused_ui(&self) -> bool {
    self.reap_ui_releases();
    self.ui_tree.borrow_mut().blur_focused()
  }

  pub fn dismiss_focused_ui(&self) -> bool {
    self.reap_ui_releases();
    self.ui_tree.borrow_mut().dismiss_focused_element()
  }

  fn reap_ui_releases(&self) -> bool {
    let releases: Vec<_> = self.ui_releases.borrow().try_iter().collect();
    if releases.is_empty() {
      return false;
    }
    let mut tree = self.ui_tree.borrow_mut();
    let mut changed = false;
    for release in releases {
      let released = match release.kind {
        UiHandleKind::Node => tree.release_node_handle(release.id),
        UiHandleKind::Element => tree.remove_element(release.id),
      };
      changed |= released;
    }
    changed
  }

  pub fn invoke_ui_key_callback(
    &self,
    callback: &Function,
    handle: Table,
    key: crossterm::event::KeyEvent,
  ) -> LuaResult<Vec<RuntimeCommand>> {
    let key_stroke = KeyStroke::from_crossterm(key)
      .ok_or_else(|| mlua::Error::external("unsupported UI key event"))?;
    let key_value = self.lua.create_table()?;
    key_value.set("key", crate::input::key_notation::key_stroke_to_string(&key_stroke))?;
    key_value.set("ctrl", key_stroke.ctrl)?;
    key_value.set("alt", key_stroke.alt)?;
    key_value.set("shift", key_stroke.shift)?;
    key_value.set("is_repeat", key.kind == crossterm::event::KeyEventKind::Repeat)?;
    self.invoke_callback(callback, (handle, key_value))
  }

  pub fn launch_state(&self) -> LuaResult<LaunchState> {
    Ok(LaunchState {
      command: self.launch.get("command")?,
      session_name: self.launch.get("session")?,
      create_if_missing: self.launch.get("create_if_missing")?,
      fail_if_exists: self.launch.get("fail_if_exists")?,
      require_session: self.launch.get("require_session")?,
      working_directory: PathBuf::from(self.launch.get::<String>("working_directory")?),
    })
  }

  pub fn invoke_event_handler(
    &self,
    callback: &LuaCallback,
    event: &AppEvent,
  ) -> LuaResult<Vec<RuntimeCommand>> {
    let value = self.event_value(event)?;
    self.invoke_callback(&callback.function, value)
  }

  pub fn invoke_key_callback(&self, callback: &Function) -> LuaResult<Vec<RuntimeCommand>> {
    self.invoke_callback(callback, ())
  }

  fn invoke_callback<A: mlua::IntoLuaMulti>(
    &self,
    callback: &Function,
    args: A,
  ) -> LuaResult<Vec<RuntimeCommand>> {
    if self.reap_ui_releases() {
      self.commands.borrow_mut().push_back(RuntimeCommand::Redraw);
    }
    self.in_event_callback.set(true);
    let result = callback.call::<()>(args);
    self.in_event_callback.set(false);
    if result.is_err() {
      self.commands.borrow_mut().clear();
    }
    let released = self.reap_ui_releases();
    if released && result.is_ok() {
      self.commands.borrow_mut().push_back(RuntimeCommand::Redraw);
    }
    result?;
    Ok(self.commands.borrow_mut().drain(..).collect())
  }

  pub fn picker_labels(&self, picker_id: u64) -> Option<Vec<String>> {
    self
      .pending_pickers
      .borrow()
      .get(&picker_id)
      .map(|picker| picker.labels.clone())
  }

  pub fn complete_picker(
    &self,
    picker_id: u64,
    selected_index: Option<usize>,
  ) -> LuaResult<Vec<RuntimeCommand>> {
    let picker = self
      .pending_pickers
      .borrow_mut()
      .remove(&picker_id)
      .ok_or_else(|| mlua::Error::external("picker is no longer pending"))?;
    let value = selected_index
      .and_then(|index| picker.values.get(index).cloned())
      .unwrap_or(Value::Nil);
    let result = self.invoke_callback(&picker.callback, value);
    drop(picker);
    let released = self.reap_ui_releases();
    match result {
      Ok(mut commands) => {
        if released {
          commands.push(RuntimeCommand::Redraw);
        }
        Ok(commands)
      }
      Err(error) => Err(error),
    }
  }

  pub fn sync_options(&self) -> LuaResult<UiOptions> {
    let ui = self.options.get::<Option<Table>>("ui")?;
    let Some(ui) = ui else {
      return Ok(UiOptions::default());
    };
    let anchors = ui.get::<Option<Table>>("anchors")?;
    let which = ui.get::<Option<Table>>("which")?;
    let pty = ui.get::<Option<Table>>("pty")?;
    Ok(UiOptions {
      top: read_anchor(anchors.as_ref(), "top")?,
      bottom: read_anchor(anchors.as_ref(), "bottom")?,
      which: read_which_key(which.as_ref())?,
      pty: read_pty(pty.as_ref())?,
    })
  }

  pub fn render_bar_part(&self, anchor: &str, part: &str) -> LuaResult<Option<Vec<BarSegment>>> {
    let context = |step: &str| format!("bar {anchor}.{part}: {step}");
    self
      .refresh_state()
      .map_err(|error| error.context(context("refresh state")))?;
    let Some(ui) = self
      .options
      .get::<Option<Table>>("ui")
      .map_err(|error| error.context(context("read UI options")))?
    else {
      return Ok(None);
    };
    let Some(bars) = ui
      .get::<Option<Table>>("bars")
      .map_err(|error| error.context(context("read bars")))?
    else {
      return Ok(None);
    };
    let Some(bar) = bars
      .get::<Option<Table>>(anchor)
      .map_err(|error| error.context(context("read anchor")))?
    else {
      return Ok(None);
    };
    let content = bar
      .get::<Value>(part)
      .map_err(|error| error.context(context("read content")))?;
    if matches!(content, Value::Nil) {
      return Ok(None);
    }
    let content = match content {
      Value::Function(function) => function
        .call::<Value>(self.globals.clone())
        .map_err(|error| error.context(context("call content provider")))?,
      value => value,
    };
    let mut rendered = Vec::new();
    for (mut segment, segment_table) in self
      .parse_segments(content)
      .map_err(|error| error.context(context("parse content segments")))?
    {
      if segment.style.is_none()
        && let Value::Function(function) = ui
          .get::<Value>("style")
          .map_err(|error| error.context(context("read default style provider")))?
      {
        self
          .refresh_state()
          .map_err(|error| error.context(context("refresh state before style callback")))?;
        let value = function
          .call::<Value>((segment_table, self.globals.clone()))
          .map_err(|error| error.context(context("call default style provider")))?;
        segment.style =
          parse_style(value).map_err(|error| error.context(context("parse default style")))?;
      }
      rendered.push(segment);
    }
    Ok(Some(rendered))
  }

  pub fn apply_default_style(&self, segment: &mut BarSegment) -> LuaResult<()> {
    self.refresh_state()?;
    if segment.style.is_some() {
      return Ok(());
    }
    let Some(ui) = self.options.get::<Option<Table>>("ui")? else {
      return Ok(());
    };
    if let Value::Function(function) = ui.get::<Value>("style")? {
      let value = self.lua.create_table()?;
      value.set("text", segment.text.as_str())?;
      value.set("role", segment.role.as_deref())?;
      let style = function.call::<Value>((value, self.globals.clone()))?;
      segment.style = parse_style(style)?;
    }
    Ok(())
  }

  fn refresh_state(&self) -> LuaResult<()> {
    let snapshot = self.snapshot.borrow().clone();
    self.sync_state(&snapshot)
  }

  fn event_value(&self, event: &AppEvent) -> LuaResult<Table> {
    let value = self.lua.create_table()?;
    value.set("name", event.name.as_str())?;
    value.set("session_number", event.session_number.clone())?;
    value.set("workspace_id", event.workspace_id)?;
    value.set("workspace_index", event.workspace_index)?;
    value.set("previous_workspace_id", event.previous_workspace_id)?;
    value.set("tab_id", event.tab_id)?;
    value.set("index", event.index)?;
    value.set("previous_index", event.previous_index)?;
    value.set("title", event.title.clone())?;
    value.set("exit_code", event.exit_code)?;
    value.set("rows", event.rows)?;
    value.set("columns", event.columns)?;
    value.set("active", event.active)?;
    Ok(value)
  }

  fn parse_segments(&self, content: Value) -> LuaResult<Vec<(BarSegment, Table)>> {
    let values = match content {
      Value::String(text) => {
        let segment = self.lua.create_table()?;
        segment.set("text", text.to_string_lossy())?;
        return Ok(vec![(
          BarSegment {
            text: segment.get("text")?,
            role: None,
            style: None,
          },
          segment,
        )]);
      }
      Value::Table(table) if table.contains_key("text")? => vec![table],
      Value::Table(table) => table
        .sequence_values::<Table>()
        .collect::<LuaResult<Vec<_>>>()?,
      Value::Nil => return Ok(Vec::new()),
      value => {
        return Err(mlua::Error::external(format!(
          "bar content must return a string or segment table, got {}",
          value.type_name()
        )));
      }
    };
    values
      .into_iter()
      .map(|table| {
        let text = table.get::<String>("text")?;
        let role = table.get::<Option<String>>("role")?;
        let style = match table.get::<Value>("style")? {
          Value::Nil => None,
          Value::Function(function) => {
            self.refresh_state()?;
            let value = function.call::<Value>((table.clone(), self.globals.clone()))?;
            parse_style(value)?
          }
          value => parse_style(value)?,
        };
        Ok((BarSegment { text, role, style }, table))
      })
      .collect()
  }
}

impl Default for UiOptions {
  fn default() -> Self {
    Self {
      top: AnchorOptions {
        height: 1,
        visible: true,
      },
      bottom: AnchorOptions {
        height: 1,
        visible: true,
      },
      which: WhichKeyOptions {
        max_columns: 7,
        max_height: WhichKeyHeight::Percent(55),
      },
      pty: PtyOptions {
        border: DrawerBorder::Plain,
        border_style: Style::default(),
      },
    }
  }
}

fn read_pty(pty: Option<&Table>) -> LuaResult<PtyOptions> {
  let defaults = PtyOptions {
    border: DrawerBorder::Plain,
    border_style: Style::default(),
  };
  let Some(pty) = pty else {
    return Ok(defaults);
  };
  let border = match pty.get::<Value>("border")? {
    Value::Nil => defaults.border,
    value => parse_drawer_border(value)?,
  };
  let border_style = pty
    .get::<Value>("border_style")
    .and_then(parse_style)?
    .unwrap_or(defaults.border_style);
  Ok(PtyOptions {
    border,
    border_style,
  })
}

fn read_which_key(which: Option<&Table>) -> LuaResult<WhichKeyOptions> {
  let defaults = WhichKeyOptions {
    max_columns: 7,
    max_height: WhichKeyHeight::Percent(55),
  };
  let Some(which) = which else {
    return Ok(defaults);
  };
  let max_columns = which
    .get::<Option<usize>>("maxcolumns")?
    .unwrap_or(defaults.max_columns)
    .max(1);
  let max_height = match which.get::<Option<Value>>("maxheight")? {
    None | Some(Value::Nil) => defaults.max_height,
    Some(Value::Integer(rows)) if rows >= 0 => WhichKeyHeight::Rows(rows as u16),
    Some(Value::Number(rows)) if rows >= 0.0 => WhichKeyHeight::Rows(rows as u16),
    Some(Value::String(value)) => {
      let value = value.to_string_lossy();
      let Some(percent) = value.strip_suffix('%') else {
        let rows = value.parse::<u16>().map_err(|_| {
          mlua::Error::external("opt.which.maxheight must be rows or a percentage string")
        })?;
        return Ok(WhichKeyOptions {
          max_columns,
          max_height: WhichKeyHeight::Rows(rows),
        });
      };
      WhichKeyHeight::Percent(
        percent.trim().parse::<u16>().map_err(|_| {
          mlua::Error::external("opt.which.maxheight percentage must be an integer")
        })?,
      )
    }
    Some(_) => {
      return Err(mlua::Error::external("opt.which.maxheight must be rows or a percentage string"));
    }
  };
  Ok(WhichKeyOptions {
    max_columns,
    max_height,
  })
}

fn read_anchor(anchors: Option<&Table>, name: &str) -> LuaResult<AnchorOptions> {
  let defaults = AnchorOptions {
    height: 1,
    visible: true,
  };
  let Some(anchor) = anchors
    .map(|anchors| anchors.get::<Option<Table>>(name))
    .transpose()?
    .flatten()
  else {
    return Ok(defaults);
  };
  Ok(AnchorOptions {
    height: anchor
      .get::<Option<u16>>("height")?
      .unwrap_or(defaults.height),
    visible: anchor
      .get::<Option<bool>>("visible")?
      .unwrap_or(defaults.visible),
  })
}

fn parse_style(value: Value) -> LuaResult<Option<Style>> {
  let Value::Table(table) = value else {
    return if matches!(value, Value::Nil) {
      Ok(None)
    } else {
      Err(mlua::Error::external("style must return a style table"))
    };
  };
  let mut style = Style::default();
  if let Some(fg) = table.get::<Option<Value>>("fg")? {
    style = style.fg(parse_color(fg)?);
  }
  if let Some(bg) = table.get::<Option<Value>>("bg")? {
    style = style.bg(parse_color(bg)?);
  }
  let mut modifiers = Modifier::empty();
  if table.get::<Option<bool>>("bold")?.unwrap_or(false) {
    modifiers.insert(Modifier::BOLD);
  }
  if table.get::<Option<bool>>("italic")?.unwrap_or(false) {
    modifiers.insert(Modifier::ITALIC);
  }
  if table.get::<Option<bool>>("underline")?.unwrap_or(false) {
    modifiers.insert(Modifier::UNDERLINED);
  }
  if table.get::<Option<bool>>("reverse")?.unwrap_or(false) {
    modifiers.insert(Modifier::REVERSED);
  }
  Ok(Some(style.add_modifier(modifiers)))
}

fn parse_color(value: Value) -> LuaResult<Color> {
  match value {
    Value::Integer(index) if (0..=255).contains(&index) => Ok(Color::Indexed(index as u8)),
    Value::String(value) => {
      let value = value.to_string_lossy();
      let named = match value.as_str() {
        "reset" => Some(Color::Reset),
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "dark_gray" | "dark_grey" => Some(Color::DarkGray),
        "white" => Some(Color::White),
        _ => None,
      };
      if let Some(color) = named {
        return Ok(color);
      }
      if let Some(hex) = value.strip_prefix('#')
        && hex.len() == 6
      {
        let red = u8::from_str_radix(&hex[0..2], 16);
        let green = u8::from_str_radix(&hex[2..4], 16);
        let blue = u8::from_str_radix(&hex[4..6], 16);
        if let (Ok(red), Ok(green), Ok(blue)) = (red, green, blue) {
          return Ok(Color::Rgb(red, green, blue));
        }
      }
      Err(mlua::Error::external(format!("invalid color '{value}'")))
    }
    _ => Err(mlua::Error::external(
      "color must be a named color, RGB hex string, or palette index 0..255",
    )),
  }
}

pub fn load_config(source: &str) -> LuaResult<AppConfig> {
  load_config_with_context(source, &[], LaunchState::default())
}

fn create_weak_table(lua: &Lua, mode: &str) -> LuaResult<Table> {
  let table = lua.create_table()?;
  let metatable = lua.create_table()?;
  metatable.set("__mode", mode)?;
  table.set_metatable(Some(metatable))?;
  Ok(table)
}

pub fn load_config_with_context(
  source: &str,
  arguments: &[String],
  initial_launch: LaunchState,
) -> LuaResult<AppConfig> {
  let lua = Lua::new();
  let keymap = Rc::new(RefCell::new(KeyMap::default()));
  let handlers = Rc::new(RefCell::new(HashMap::<String, Vec<RegisteredCallback>>::new()));
  let commands = Rc::new(RefCell::new(VecDeque::<RuntimeCommand>::new()));
  let in_event_callback = Rc::new(Cell::new(false));
  let pending_pickers = Rc::new(RefCell::new(HashMap::<u64, PendingPicker>::new()));
  let ui_tree = Rc::new(RefCell::new(UiTree::default()));
  let ui_handles = create_weak_table(&lua, "v")?;
  let ui_leases = create_weak_table(&lua, "k")?;
  let (ui_release_sender, ui_releases) = std::sync::mpsc::channel();
  let next_picker_id = Rc::new(Cell::new(1_u64));
  let config_loading = Rc::new(Cell::new(true));
  let globals = lua.create_table()?;
  lua.globals().set("g", globals.clone())?;
  lua.globals().set("global", globals.clone())?;
  lua.globals().set("inf", lua.create_table()?)?;
  let zmux = lua.create_table()?;
  zmux.set("version", env!("CARGO_PKG_VERSION"))?;
  zmux.set("config_api_version", CONFIG_API_VERSION)?;
  lua.globals().set("zmux", zmux)?;
  let arguments_table = lua.create_table_with_capacity(arguments.len(), 0)?;
  for (index, argument) in arguments.iter().enumerate() {
    arguments_table.set(index + 1, argument.as_str())?;
  }
  lua.globals().set("args", arguments_table)?;
  let launch = lua.create_table()?;
  launch.set("command", initial_launch.command)?;
  launch.set("session", initial_launch.session_name)?;
  launch.set("create_if_missing", initial_launch.create_if_missing)?;
  launch.set("fail_if_exists", initial_launch.fail_if_exists)?;
  launch.set("require_session", initial_launch.require_session)?;
  launch.set("working_directory", initial_launch.working_directory.to_string_lossy().as_ref())?;
  lua.globals().set("launch", launch.clone())?;

  let bind = {
    let keymap = Rc::clone(&keymap);
    let config_loading = Rc::clone(&config_loading);
    lua.create_function(move |_, (key, callback, options): (String, Value, Option<Table>)| {
      if !config_loading.get() {
        return Err(mlua::Error::external(
          "cmd.key.bind is only available while loading configuration",
        ));
      }
      let sequence = parse_key_notation(&key).map_err(mlua::Error::external)?;
      if matches!(callback, Value::Nil) {
        keymap.borrow_mut().unbind(&sequence);
        return Ok(());
      }
      let Value::Function(callback) = callback else {
        return Err(mlua::Error::external("cmd.key.bind callback must be a function or nil"));
      };
      let desc = options
        .map(|options| options.get::<Option<String>>("desc"))
        .transpose()?
        .flatten();
      keymap
        .borrow_mut()
        .bind(sequence, callback, desc)
        .map_err(mlua::Error::external)
    })?
  };
  let on = {
    let handlers = Rc::clone(&handlers);
    let config_loading = Rc::clone(&config_loading);
    lua.create_function(
      move |_, (events, callback, options): (Value, Function, Option<Table>)| {
        if !config_loading.get() {
          return Err(mlua::Error::external(
            "cmd.event.on is only available while loading configuration",
          ));
        }
        let names = match events {
          Value::String(name) => vec![name.to_string_lossy()],
          Value::Table(events) => events
            .sequence_values::<Value>()
            .map(|event| match event? {
              Value::String(name) => Ok(name.to_string_lossy()),
              _ => Err(mlua::Error::external("cmd.event.on event names must be strings")),
            })
            .collect::<LuaResult<Vec<_>>>()?,
          other => {
            return Err(mlua::Error::external(format!(
              "cmd.event.on expects an event name or an array of event names, got {}",
              other.type_name()
            )));
          }
        };
        for name in &names {
          if !valid_event_name(name) {
            return Err(mlua::Error::external(format!("unknown event: {name}")));
          }
        }
        let interrupt = options
          .map(|options| options.get::<Option<bool>>("interrupt"))
          .transpose()?
          .flatten()
          .unwrap_or(false);
        let mut handlers = handlers.borrow_mut();
        let mut registered_names = Vec::new();
        for name in names {
          if registered_names.contains(&name) {
            continue;
          }
          registered_names.push(name.clone());
          handlers.entry(name).or_default().push(RegisteredCallback {
            function: callback.clone(),
            interrupt,
          });
        }
        Ok(())
      },
    )?
  };
  let cmd = lua.create_table()?;
  let key = lua.create_table()?;
  key.set("bind", bind)?;
  let event = lua.create_table()?;
  event.set("on", on)?;
  let ui = lua.create_table()?;
  let client = lua.create_table()?;
  let server = lua.create_table()?;
  let session = lua.create_table()?;
  let workspace = lua.create_table()?;
  let tab = lua.create_table()?;
  cmd.set("key", key)?;
  cmd.set("event", event)?;
  cmd.set("ui", ui.clone())?;
  cmd.set("client", client.clone())?;
  cmd.set("server", server.clone())?;
  cmd.set("session", session.clone())?;
  cmd.set("workspace", workspace.clone())?;
  cmd.set("tab", tab.clone())?;
  {
    let pending_pickers = Rc::clone(&pending_pickers);
    let next_picker_id = Rc::clone(&next_picker_id);
    let commands = Rc::clone(&commands);
    let in_event_callback = Rc::clone(&in_event_callback);
    ui.set(
      "pick",
      lua.create_function(move |_, (items, callback): (Table, Function)| {
        ensure_command_context(&in_event_callback)?;
        let picker_id = next_picker_id.get();
        next_picker_id.set(
          picker_id
            .checked_add(1)
            .ok_or_else(|| mlua::Error::external("picker identifier space exhausted"))?,
        );
        let mut labels = Vec::new();
        let mut values = Vec::new();
        for item in items.sequence_values::<Value>() {
          let item = item?;
          let (label, value) = picker_item(item)?;
          labels.push(label);
          values.push(value);
        }
        pending_pickers
          .borrow_mut()
          .insert(picker_id, PendingPicker {
            labels,
            values,
            callback,
          });
        commands
          .borrow_mut()
          .push_back(RuntimeCommand::OpenPicker(picker_id));
        Ok(())
      })?,
    )?;
  }
  let ui_api = UiApiContext {
    tree: Rc::clone(&ui_tree),
    handles: ui_handles.clone(),
    leases: ui_leases.clone(),
    release_sender: ui_release_sender,
    commands: Rc::clone(&commands),
    config_loading: Rc::clone(&config_loading),
    in_callback: Rc::clone(&in_event_callback),
  };
  register_ui_api(&lua, &ui, &ui_api)?;
  register_command(&lua, &ui, &commands, &in_event_callback, "redraw", RuntimeCommand::Redraw)?;
  register_command(&lua, &client, &commands, &in_event_callback, "detach", RuntimeCommand::Detach)?;
  register_command(
    &lua,
    &server,
    &commands,
    &in_event_callback,
    "stop",
    RuntimeCommand::StopServer,
  )?;
  register_command(
    &lua,
    &session,
    &commands,
    &in_event_callback,
    "kill",
    RuntimeCommand::KillSession,
  )?;
  register_command(
    &lua,
    &session,
    &commands,
    &in_event_callback,
    "switch",
    RuntimeCommand::OpenSessionPicker,
  )?;
  register_command(
    &lua,
    &workspace,
    &commands,
    &in_event_callback,
    "create",
    RuntimeCommand::CreateWorkspace,
  )?;
  register_command(
    &lua,
    &workspace,
    &commands,
    &in_event_callback,
    "edit",
    RuntimeCommand::EditWorkspace,
  )?;
  register_command(
    &lua,
    &workspace,
    &commands,
    &in_event_callback,
    "close",
    RuntimeCommand::CloseWorkspace,
  )?;
  register_command(
    &lua,
    &workspace,
    &commands,
    &in_event_callback,
    "next",
    RuntimeCommand::NextWorkspace,
  )?;
  register_command(
    &lua,
    &workspace,
    &commands,
    &in_event_callback,
    "previous",
    RuntimeCommand::PreviousWorkspace,
  )?;
  {
    let commands = Rc::clone(&commands);
    let in_event_callback = Rc::clone(&in_event_callback);
    workspace.set(
      "select",
      lua.create_function(move |_, id: u64| {
        ensure_command_context(&in_event_callback)?;
        commands
          .borrow_mut()
          .push_back(RuntimeCommand::SelectWorkspace(id));
        Ok(())
      })?,
    )?;
  }
  register_command(&lua, &tab, &commands, &in_event_callback, "create", RuntimeCommand::NewTab)?;
  register_command(&lua, &tab, &commands, &in_event_callback, "close", RuntimeCommand::CloseTab)?;
  register_command(&lua, &tab, &commands, &in_event_callback, "next", RuntimeCommand::NextTab)?;
  register_command(
    &lua,
    &tab,
    &commands,
    &in_event_callback,
    "previous",
    RuntimeCommand::PreviousTab,
  )?;
  {
    let commands = Rc::clone(&commands);
    let in_event_callback = Rc::clone(&in_event_callback);
    tab.set(
      "select",
      lua.create_function(move |_, index: usize| {
        if index == 0 {
          return Err(mlua::Error::external("tab indexes are one-based"));
        }
        ensure_command_context(&in_event_callback)?;
        commands
          .borrow_mut()
          .push_back(RuntimeCommand::SelectTab(index));
        Ok(())
      })?,
    )?;
  }
  lua.globals().set("cmd", cmd)?;

  let options = lua.create_table()?;
  options.set("cursor_shape", "block")?;
  options.set("prefix_key", "<C-Space>")?;
  options.set("sequence_timeout_ms", 700_u64)?;
  let ui = lua.create_table()?;
  let anchors = lua.create_table()?;
  for name in ["top", "bottom"] {
    let anchor = lua.create_table()?;
    anchor.set("height", 1)?;
    anchor.set("visible", true)?;
    anchors.set(name, anchor)?;
  }
  ui.set("anchors", anchors)?;
  let which = lua.create_table()?;
  which.set("maxcolumns", 7_u64)?;
  which.set("maxheight", "55%")?;
  ui.set("which", which)?;
  let pty = lua.create_table()?;
  pty.set("border", "plain")?;
  ui.set("pty", pty)?;
  let bars = lua.create_table()?;
  for name in ["top", "bottom"] {
    bars.set(name, lua.create_table()?)?;
  }
  ui.set("bars", bars)?;
  options.set("ui", ui)?;
  lua.globals().set("opt", options.clone())?;
  lua.load(source).exec()?;
  config_loading.set(false);

  let config_api_version = options.get::<Option<u32>>("config_version")?;
  if let Some(version) = config_api_version
    && version != CONFIG_API_VERSION
  {
    return Err(mlua::Error::external(format!(
      "config API version {version} is not supported; this program supports version {CONFIG_API_VERSION}"
    )));
  }

  let cursor_shape = options
    .get::<String>("cursor_shape")?
    .parse::<CursorShape>()
    .map_err(mlua::Error::external)?;
  let prefix_key = options
    .get::<String>("prefix_key")
    .and_then(|value| normalize_prefix_key(&value).map_err(mlua::Error::external))?;
  let default_shell = options
    .get::<Option<String>>("default_shell")?
    .map(PathBuf::from);
  let timeout_ms = options.get::<u64>("sequence_timeout_ms")?;
  let keymap = keymap.borrow().clone();
  Ok(AppConfig {
    config_api_version,
    keymap,
    cursor_shape,
    default_shell,
    prefix_key,
    sequence_timeout: Duration::from_millis(timeout_ms),
    lua: LuaConfig {
      lua,
      options,
      globals,
      snapshot: RefCell::new(AppSnapshot::default()),
      handlers,
      commands,
      in_event_callback,
      pending_pickers,
      ui_tree,
      ui_handles,
      _ui_leases: ui_leases,
      ui_releases: RefCell::new(ui_releases),
      launch,
    },
  })
}

fn register_ui_api(lua: &Lua, ui: &Table, context: &UiApiContext) -> LuaResult<()> {
  let text = {
    let context = context.clone();
    lua.create_function(move |lua, options: Table| {
      ensure_ui_mutation_context(&context.config_loading, &context.in_callback)?;
      let text = options
        .get::<Option<String>>("text")?
        .ok_or_else(|| mlua::Error::external("Text nodes require a 'text' string"))?;
      let style = options
        .get::<Value>("style")
        .and_then(parse_optional_style)?
        .unwrap_or_default();
      let handle = make_text_handle(lua, &context, text, style)?;
      finish_ui_mutation(true, &context.config_loading, &context.commands);
      Ok(handle)
    })?
  };
  let row = create_layout_constructor(lua, context, LayoutAxis::Horizontal)?;
  let column = create_layout_constructor(lua, context, LayoutAxis::Vertical)?;
  let create = {
    let context = context.clone();
    lua.create_function(move |lua, options: Table| {
      ensure_ui_mutation_context(&context.config_loading, &context.in_callback)?;
      let handle = create_ui_element_handle(lua, &context, options)?;
      finish_ui_mutation(true, &context.config_loading, &context.commands);
      Ok(handle)
    })?
  };
  ui.set("text", text)?;
  ui.set("row", row)?;
  ui.set("column", column)?;
  ui.set("create", create)?;
  Ok(())
}

fn ensure_ui_mutation_context(
  config_loading: &Cell<bool>,
  in_callback: &Cell<bool>,
) -> LuaResult<()> {
  if config_loading.get() {
    Ok(())
  } else {
    ensure_command_context(in_callback)
  }
}

fn finish_ui_mutation(
  changed: bool,
  config_loading: &Cell<bool>,
  commands: &RefCell<VecDeque<RuntimeCommand>>,
) {
  if changed && !config_loading.get() {
    commands.borrow_mut().push_back(RuntimeCommand::Redraw);
  }
}

fn parse_optional_style(value: Value) -> LuaResult<Option<Style>> { parse_style(value) }

fn track_ui_handle(
  lua: &Lua,
  leases: &Table,
  sender: &Sender<UiHandleRelease>,
  handle: &Table,
  kind: UiHandleKind,
  id: u64,
) -> LuaResult<()> {
  let lease = lua.create_userdata(UiHandleLease {
    release: UiHandleRelease { kind, id },
    sender: sender.clone(),
  })?;
  leases.set(handle.clone(), lease)
}

fn ui_node_id(handle: &Table) -> LuaResult<u64> {
  handle
    .get::<Option<u64>>("__zmux_ui_node")?
    .ok_or_else(|| mlua::Error::external("expected a UI node handle"))
}

fn parse_child_size(value: Value) -> LuaResult<crate::ui::ChildSize> {
  match value {
    Value::Nil => Ok(crate::ui::ChildSize::Auto),
    Value::Integer(size) if (0..=i64::from(u16::MAX)).contains(&size) => {
      Ok(crate::ui::ChildSize::Fixed(size as u16))
    }
    Value::String(value) => match value.to_string_lossy().as_str() {
      "auto" => Ok(crate::ui::ChildSize::Auto),
      "fill" => Ok(crate::ui::ChildSize::Fill),
      _ => Err(mlua::Error::external("size must be 'auto', 'fill', or a cell count")),
    },
    _ => Err(mlua::Error::external("size must be 'auto', 'fill', or a cell count")),
  }
}

fn parse_ui_children(children: &Table) -> LuaResult<Vec<UiChild>> {
  children
    .sequence_values::<Value>()
    .map(|value| {
      let value = value?;
      let Value::Table(spec) = value else {
        return Err(mlua::Error::external(
          "Layout children must be UI node handles or child specs",
        ));
      };
      if let Some(handle) = spec.get::<Option<Table>>("node")? {
        Ok(UiChild {
          node_id: ui_node_id(&handle)?,
          size: parse_child_size(spec.get::<Value>("size")?)?,
        })
      } else {
        Ok(UiChild {
          node_id: ui_node_id(&spec)?,
          size: crate::ui::ChildSize::Auto,
        })
      }
    })
    .collect()
}

fn parse_ui_keys(keys: &Table) -> LuaResult<HashMap<KeyStroke, Function>> {
  let mut bindings = HashMap::new();
  for entry in keys.clone().pairs::<Value, Value>() {
    let (key, callback) = entry?;
    let Value::String(key) = key else {
      return Err(mlua::Error::external("UI key binding names must be strings"));
    };
    let key = key.to_string_lossy();
    let sequence = parse_key_notation(&key).map_err(mlua::Error::external)?;
    if sequence.len() != 1 {
      return Err(mlua::Error::external(
        "focused UI bindings must be single direct keys; use the global leader map for sequences",
      ));
    }
    let Value::Function(callback) = callback else {
      return Err(mlua::Error::external("UI key bindings must be functions"));
    };
    let stroke = sequence[0];
    if bindings.insert(stroke, callback).is_some() {
      return Err(mlua::Error::external(format!("duplicate UI key binding '{key}'")));
    }
  }
  Ok(bindings)
}

fn ui_keymap_table(lua: &Lua, bindings: &HashMap<KeyStroke, Function>) -> LuaResult<Table> {
  let table = lua.create_table_with_capacity(bindings.len(), 0)?;
  for (key, callback) in bindings {
    table.set(crate::input::key_notation::key_stroke_to_string(key), callback.clone())?;
  }
  Ok(table)
}

fn parse_drawer_side(value: &str) -> LuaResult<DrawerSide> {
  match value {
    "left" => Ok(DrawerSide::Left),
    "right" => Ok(DrawerSide::Right),
    _ => Err(mlua::Error::external("drawer side must be 'left' or 'right'")),
  }
}

fn parse_ui_size(value: Value) -> LuaResult<UiSize> {
  match value {
    Value::Nil => Ok(UiSize::Auto),
    Value::Integer(size) if (0..=i64::from(u16::MAX)).contains(&size) => {
      Ok(UiSize::Fixed(size as u16))
    }
    Value::String(size) if size.to_string_lossy() == "auto" => Ok(UiSize::Auto),
    _ => Err(mlua::Error::external("UI size must be 'auto' or a non-negative cell count")),
  }
}

fn parse_ui_coordinate(value: Value, property: &str) -> LuaResult<u16> {
  match value {
    Value::Nil => Ok(0),
    Value::Integer(value) if (0..=i64::from(u16::MAX)).contains(&value) => Ok(value as u16),
    _ => Err(mlua::Error::external(format!(
      "UI coordinate '{property}' must be a non-negative cell count"
    ))),
  }
}

fn parse_toast_position(value: &str) -> LuaResult<ToastPosition> {
  match value {
    "top_right" => Ok(ToastPosition::TopRight),
    "top_center" => Ok(ToastPosition::TopCenter),
    "bottom_right" => Ok(ToastPosition::BottomRight),
    "bottom_center" => Ok(ToastPosition::BottomCenter),
    _ => Err(mlua::Error::external(
      "Toast position must be 'top_right', 'top_center', 'bottom_right', or 'bottom_center'",
    )),
  }
}

fn parse_drawer_border(value: Value) -> LuaResult<DrawerBorder> {
  let Value::String(value) = value else {
    return Err(mlua::Error::external("UI border must be a Ratatui border preset"));
  };
  match value.to_string_lossy().as_str() {
    "none" => Ok(DrawerBorder::None),
    "plain" => Ok(DrawerBorder::Plain),
    "rounded" => Ok(DrawerBorder::Rounded),
    "double" => Ok(DrawerBorder::Double),
    "thick" => Ok(DrawerBorder::Thick),
    "light_double_dashed" => Ok(DrawerBorder::LightDoubleDashed),
    "heavy_double_dashed" => Ok(DrawerBorder::HeavyDoubleDashed),
    "light_triple_dashed" => Ok(DrawerBorder::LightTripleDashed),
    "heavy_triple_dashed" => Ok(DrawerBorder::HeavyTripleDashed),
    "light_quadruple_dashed" => Ok(DrawerBorder::LightQuadrupleDashed),
    "heavy_quadruple_dashed" => Ok(DrawerBorder::HeavyQuadrupleDashed),
    "quadrant_inside" => Ok(DrawerBorder::QuadrantInside),
    "quadrant_outside" => Ok(DrawerBorder::QuadrantOutside),
    value => Err(mlua::Error::external(format!("unknown UI border preset '{value}'"))),
  }
}

fn make_text_handle(
  lua: &Lua,
  context: &UiApiContext,
  text: String,
  style: Style,
) -> LuaResult<Table> {
  let id = context
    .tree
    .borrow_mut()
    .create_text(text, style)
    .map_err(mlua::Error::external)?;
  let handle = lua.create_table()?;
  handle.set("__zmux_ui_node", id)?;
  let set = {
    let tree = Rc::clone(&context.tree);
    let commands = Rc::clone(&context.commands);
    let config_loading = Rc::clone(&context.config_loading);
    let in_callback = Rc::clone(&context.in_callback);
    lua.create_function(move |_, (property, value): (String, Value)| {
      ensure_ui_mutation_context(&config_loading, &in_callback)?;
      let changed = match property.as_str() {
        "text" => {
          let Value::String(value) = value else {
            return Err(mlua::Error::external("Text 'text' property must be a string"));
          };
          tree
            .borrow_mut()
            .set_text(id, value.to_string_lossy())
            .map_err(mlua::Error::external)?
        }
        "style" => {
          let style = parse_style(value)?.unwrap_or_default();
          tree
            .borrow_mut()
            .set_text_style(id, style)
            .map_err(mlua::Error::external)?
        }
        _ => return Err(mlua::Error::external(format!("unknown Text property '{property}'"))),
      };
      finish_ui_mutation(changed, &config_loading, &commands);
      Ok(())
    })?
  };
  handle.set("set", set)?;
  track_ui_handle(lua, &context.leases, &context.release_sender, &handle, UiHandleKind::Node, id)?;
  Ok(handle)
}

fn create_layout_constructor(
  lua: &Lua,
  context: &UiApiContext,
  axis: LayoutAxis,
) -> LuaResult<Function> {
  let context = context.clone();
  lua.create_function(move |lua, children: Table| {
    ensure_ui_mutation_context(&context.config_loading, &context.in_callback)?;
    let children = parse_ui_children(&children)?;
    let id = context
      .tree
      .borrow_mut()
      .create_layout(axis, children)
      .map_err(mlua::Error::external)?;
    let handle = lua.create_table()?;
    handle.set("__zmux_ui_node", id)?;
    let set = {
      let tree = Rc::clone(&context.tree);
      let commands = Rc::clone(&context.commands);
      let config_loading = Rc::clone(&context.config_loading);
      let in_callback = Rc::clone(&context.in_callback);
      lua.create_function(move |_, (property, value): (String, Value)| {
        ensure_ui_mutation_context(&config_loading, &in_callback)?;
        if property != "children" {
          return Err(mlua::Error::external(format!("unknown Layout property '{property}'")));
        }
        let Value::Table(children) = value else {
          return Err(mlua::Error::external("Layout 'children' property must be an array"));
        };
        let children = parse_ui_children(&children)?;
        let changed = tree
          .borrow_mut()
          .set_children(id, children)
          .map_err(mlua::Error::external)?;
        finish_ui_mutation(changed, &config_loading, &commands);
        Ok(())
      })?
    };
    handle.set("set", set)?;
    track_ui_handle(
      lua,
      &context.leases,
      &context.release_sender,
      &handle,
      UiHandleKind::Node,
      id,
    )?;
    finish_ui_mutation(true, &context.config_loading, &context.commands);
    Ok(handle)
  })
}

fn create_ui_element_handle(lua: &Lua, context: &UiApiContext, options: Table) -> LuaResult<Table> {
  let type_name = options
    .get::<Option<String>>("type")?
    .unwrap_or_else(|| "drawer".to_owned());
  let element_type = match type_name.as_str() {
    "drawer" => UiElementType::Drawer {
      side: parse_drawer_side(
        &options
          .get::<Option<String>>("side")?
          .unwrap_or_else(|| "left".to_owned()),
      )?,
      width: parse_ui_size(options.get::<Value>("width")?)?,
    },
    "modal" => UiElementType::Modal {
      width: parse_ui_size(options.get::<Value>("width")?)?,
      height: parse_ui_size(options.get::<Value>("height")?)?,
    },
    "popover" => UiElementType::Popover {
      x: parse_ui_coordinate(options.get::<Value>("x")?, "x")?,
      y: parse_ui_coordinate(options.get::<Value>("y")?, "y")?,
      width: parse_ui_size(options.get::<Value>("width")?)?,
      height: parse_ui_size(options.get::<Value>("height")?)?,
    },
    "bottom_panel" => UiElementType::BottomPanel {
      height: parse_ui_size(options.get::<Value>("height")?)?,
    },
    "toast" => {
      let position = parse_toast_position(
        &options
          .get::<Option<String>>("position")?
          .unwrap_or_else(|| "top_right".to_owned()),
      )?;
      let duration = Duration::from_millis(options.get::<Option<u64>>("duration")?.unwrap_or(3000));
      UiElementType::Toast { position, duration }
    }
    _ => {
      return Err(mlua::Error::external(
        "cmd.ui.create type must be 'drawer', 'modal', 'popover', 'bottom_panel', or 'toast'",
      ));
    }
  };
  let title = options.get::<Option<String>>("title")?.unwrap_or_default();
  let border = match options.get::<Value>("border")? {
    Value::Nil => DrawerBorder::Plain,
    value => parse_drawer_border(value)?,
  };
  let border_style = options
    .get::<Value>("border_style")
    .and_then(parse_optional_style)?
    .unwrap_or_default();
  let content = options
    .get::<Option<Table>>("content")?
    .ok_or_else(|| mlua::Error::external("UI elements require a 'content' UI node"))?;
  let content_id = ui_node_id(&content)?;
  let keys = options
    .get::<Option<Table>>("keys")?
    .map(|keys| parse_ui_keys(&keys))
    .transpose()?
    .unwrap_or_default();
  if matches!(element_type, UiElementType::Toast { .. }) && !keys.is_empty() {
    return Err(mlua::Error::external("Toast elements do not support key bindings"));
  }
  let is_toast = matches!(element_type, UiElementType::Toast { .. });
  let id = Rc::new(Cell::new(0_u64));
  let handle = make_ui_element_handle(lua, context, Rc::clone(&id), is_toast)?;
  let element_id = context
    .tree
    .borrow_mut()
    .create_element(UiElementOptions {
      element_type,
      title,
      border,
      border_style,
      content_id,
      visible: options.get::<Option<bool>>("visible")?.unwrap_or(false),
    })
    .map_err(mlua::Error::external)?;
  id.set(element_id);
  handle.set("__zmux_ui_element", element_id)?;
  handle.set("__zmux_ui_keys", ui_keymap_table(lua, &keys)?)?;
  context.handles.set(element_id, handle.clone())?;
  track_ui_handle(
    lua,
    &context.leases,
    &context.release_sender,
    &handle,
    UiHandleKind::Element,
    element_id,
  )?;
  Ok(handle)
}

fn make_ui_element_handle(
  lua: &Lua,
  context: &UiApiContext,
  id: Rc<Cell<u64>>,
  is_toast: bool,
) -> LuaResult<Table> {
  let handle = lua.create_table()?;
  let set = {
    let tree = Rc::clone(&context.tree);
    let handles = context.handles.clone();
    let id_for_keys = Rc::clone(&id);
    let commands = Rc::clone(&context.commands);
    let config_loading = Rc::clone(&context.config_loading);
    let in_callback = Rc::clone(&context.in_callback);
    let id = Rc::clone(&id);
    lua.create_function(move |lua, (property, value): (String, Value)| {
      ensure_ui_mutation_context(&config_loading, &in_callback)?;
      if property == "keys" {
        if is_toast {
          return Err(mlua::Error::external("Toast elements do not support key bindings"));
        }
        let Value::Table(keys) = value else {
          return Err(mlua::Error::external("UI element 'keys' property must be a table"));
        };
        let keys = parse_ui_keys(&keys)?;
        let handle = handles
          .get::<Option<Table>>(id_for_keys.get())?
          .ok_or_else(|| mlua::Error::external("UI element handle is no longer reachable"))?;
        handle.set("__zmux_ui_keys", ui_keymap_table(lua, &keys)?)?;
        return Ok(());
      }
      let (property_kind, value) = match property.as_str() {
        "side" => {
          let Value::String(value) = value else {
            return Err(mlua::Error::external("UI element 'side' property must be a string"));
          };
          (
            UiElementProperty::Side,
            UiElementValue::Side(parse_drawer_side(&value.to_string_lossy())?),
          )
        }
        "width" => (UiElementProperty::Width, UiElementValue::Size(parse_ui_size(value)?)),
        "height" => (UiElementProperty::Height, UiElementValue::Size(parse_ui_size(value)?)),
        "x" => (UiElementProperty::X, UiElementValue::Coordinate(parse_ui_coordinate(value, "x")?)),
        "y" => (UiElementProperty::Y, UiElementValue::Coordinate(parse_ui_coordinate(value, "y")?)),
        "position" => {
          let Value::String(value) = value else {
            return Err(mlua::Error::external("Toast 'position' property must be a string"));
          };
          (
            UiElementProperty::Position,
            UiElementValue::Position(parse_toast_position(&value.to_string_lossy())?),
          )
        }
        "duration" => {
          let Value::Integer(value) = value else {
            return Err(mlua::Error::external(
              "Toast 'duration' property must be an integer number of milliseconds",
            ));
          };
          if value < 0 {
            return Err(mlua::Error::external("Toast 'duration' must not be negative"));
          }
          (
            UiElementProperty::Duration,
            UiElementValue::Duration(Duration::from_millis(value as u64)),
          )
        }
        "title" => {
          let Value::String(value) = value else {
            return Err(mlua::Error::external("UI element 'title' property must be a string"));
          };
          (UiElementProperty::Title, UiElementValue::Title(value.to_string_lossy()))
        }
        "border" => {
          (UiElementProperty::Border, UiElementValue::Border(parse_drawer_border(value)?))
        }
        "border_style" => {
          let style = parse_style(value)?.unwrap_or_default();
          (UiElementProperty::BorderStyle, UiElementValue::Style(style))
        }
        _ => {
          return Err(mlua::Error::external(format!("unknown UI element property '{property}'")));
        }
      };
      let changed = tree
        .borrow_mut()
        .set_element_property(id.get(), property_kind, value)
        .map_err(mlua::Error::external)?;
      finish_ui_mutation(changed, &config_loading, &commands);
      Ok(())
    })?
  };
  handle.set("set", set)?;
  for action in ["show", "hide", "toggle", "focus", "blur"] {
    let tree = Rc::clone(&context.tree);
    let commands = Rc::clone(&context.commands);
    let config_loading = Rc::clone(&context.config_loading);
    let in_callback = Rc::clone(&context.in_callback);
    let id = Rc::clone(&id);
    let action_name = action;
    handle.set(
      action,
      lua.create_function(move |_, ()| {
        ensure_ui_mutation_context(&config_loading, &in_callback)?;
        let changed = match action_name {
          "show" => tree.borrow_mut().show(id.get()),
          "hide" => tree.borrow_mut().hide(id.get()),
          "toggle" => tree.borrow_mut().toggle(id.get()),
          "focus" => tree.borrow_mut().focus(id.get()),
          "blur" => tree.borrow_mut().blur(id.get()),
          _ => unreachable!(),
        }
        .map_err(mlua::Error::external)?;
        finish_ui_mutation(changed, &config_loading, &commands);
        Ok(())
      })?,
    )?;
  }
  Ok(handle)
}

fn picker_item(value: Value) -> LuaResult<(String, Value)> {
  match value {
    Value::String(text) => Ok((text.to_string_lossy(), Value::String(text))),
    Value::Table(table) => {
      let label = table
        .get::<Option<String>>("label")?
        .or(table.get::<Option<String>>("text")?)
        .ok_or_else(|| {
          mlua::Error::external("picker table items require a string 'label' or 'text'")
        })?;
      let selected = table.get::<Value>("value")?;
      let value = if matches!(selected, Value::Nil) {
        Value::Table(table)
      } else {
        selected
      };
      Ok((label, value))
    }
    other => Err(mlua::Error::external(format!(
      "picker items must be strings or tables, got {}",
      other.type_name()
    ))),
  }
}

fn valid_event_name(name: &str) -> bool {
  matches!(
    name,
    "PreClient"
      | "ClientLoaded"
      | "session_created"
      | "session_selected"
      | "session_closed"
      | "workspace_created"
      | "workspace_closed"
      | "workspace_selected"
      | "tab_created"
      | "tab_closed"
      | "tab_selected"
      | "leader_mode_changed"
      | "terminal_title_changed"
      | "process_exited"
      | "terminal_resized"
  )
}

fn register_command(
  lua: &Lua,
  namespace: &Table,
  queue: &Rc<RefCell<VecDeque<RuntimeCommand>>>,
  in_callback: &Rc<Cell<bool>>,
  name: &str,
  command: RuntimeCommand,
) -> LuaResult<()> {
  let queue = Rc::clone(queue);
  let in_callback = Rc::clone(in_callback);
  namespace.set(
    name,
    lua.create_function(move |_, ()| {
      ensure_command_context(&in_callback)?;
      queue.borrow_mut().push_back(command.clone());
      Ok(())
    })?,
  )?;
  Ok(())
}

fn ensure_command_context(in_callback: &Cell<bool>) -> LuaResult<()> {
  if in_callback.get() {
    Ok(())
  } else {
    Err(mlua::Error::external(
      "runtime cmd functions may only be called from key or event callbacks",
    ))
  }
}

fn first_executable_line(source: &str) -> Option<&str> {
  let bytes = source.as_bytes();
  let mut index = 0;
  loop {
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
      index += 1;
    }
    if index == bytes.len() {
      return None;
    }
    if bytes.get(index..index + 2) == Some(b"--") {
      index += 2;
      if bytes.get(index) == Some(&b'[') {
        let mut delimiter_end = index + 1;
        while bytes.get(delimiter_end) == Some(&b'=') {
          delimiter_end += 1;
        }
        if bytes.get(delimiter_end) == Some(&b'[') {
          let equals = delimiter_end - index - 1;
          let closing = format!("]{}]", "=".repeat(equals));
          let content_start = delimiter_end + 1;
          let closing_offset = source.get(content_start..)?.find(&closing)?;
          index = content_start + closing_offset + closing.len();
          continue;
        }
      }
      index = bytes[index..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |newline| index + newline + 1);
      continue;
    }
    if bytes.get(index..index + 2) == Some(b"#!") {
      index = bytes[index..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |newline| index + newline + 1);
      continue;
    }
    let line_end = bytes[index..]
      .iter()
      .position(|byte| *byte == b'\n')
      .map_or(bytes.len(), |newline| index + newline);
    return source.get(index..line_end).map(str::trim);
  }
}

fn validate_file_config_api_version(source: &str) -> LuaResult<()> {
  let declaration = first_executable_line(source);
  let Some(declaration) = declaration else {
    return Err(mlua::Error::external(format!(
      "configuration must declare opt.config_version = {CONFIG_API_VERSION} as its first executable statement"
    )));
  };
  let Some(value) = declaration
    .strip_prefix("opt.config_version")
    .map(str::trim_start)
    .and_then(|value| value.strip_prefix('='))
  else {
    return Err(mlua::Error::external(format!(
      "configuration must declare opt.config_version = {CONFIG_API_VERSION} as its first executable statement"
    )));
  };
  let version = value
    .split_once("--")
    .map_or(value, |(version, _)| version)
    .trim()
    .parse::<u32>()
    .map_err(|_| {
      mlua::Error::external(format!(
        "configuration must declare opt.config_version = {CONFIG_API_VERSION} as its first executable statement"
      ))
    })?;
  if version != CONFIG_API_VERSION {
    return Err(mlua::Error::external(format!(
      "config API version {version} is not supported; this program supports version {CONFIG_API_VERSION}"
    )));
  }
  Ok(())
}

pub fn load_file_with_context(
  path: &std::path::Path,
  arguments: &[String],
  launch: LaunchState,
) -> LuaResult<AppConfig> {
  let source = std::fs::read_to_string(path).map_err(mlua::Error::external)?;
  validate_file_config_api_version(&source)?;
  let config = load_config_with_context(&source, arguments, launch)?;
  if config.config_api_version.is_none() {
    return Err(mlua::Error::external(format!(
      "configuration must declare opt.config_version = {CONFIG_API_VERSION}"
    )));
  }
  Ok(config)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn lua_exposes_program_and_config_api_versions() {
    load_config(
      "assert(type(zmux.version) == 'string' and #zmux.version > 0)\nassert(zmux.config_api_version == 1)",
    )
    .expect("runtime version metadata");
  }

  #[test]
  fn declared_config_api_version_must_be_supported() {
    let error = match load_config("opt.config_version = 2") {
      Ok(_) => panic!("unsupported config API must be rejected"),
      Err(error) => error,
    };
    assert!(error.to_string().contains("config API version 2"));
    assert!(error.to_string().contains("supports version 1"));
  }

  #[test]
  fn config_files_must_declare_the_config_api_version() {
    let directory = std::env::temp_dir().join(format!(
      "zmux-config-version-test-{}-{}",
      std::process::id(),
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_nanos(),
    ));
    std::fs::create_dir_all(&directory).expect("create temporary config directory");
    let path = directory.join("zmux.lua");
    std::fs::write(&path, "error('config without a version must not execute')")
      .expect("write temporary config");

    let error = match load_file_with_context(&path, &[], LaunchState::default()) {
      Ok(_) => panic!("config files without a version must be rejected"),
      Err(error) => error,
    };
    assert!(
      error
        .to_string()
        .contains("must declare opt.config_version = 1")
    );

    std::fs::write(&path, "opt.config_version = 2\nerror('stale config must not execute')")
      .expect("write unsupported config");
    let error = match load_file_with_context(&path, &[], LaunchState::default()) {
      Ok(_) => panic!("unsupported config files must be rejected"),
      Err(error) => error,
    };
    assert!(error.to_string().contains("config API version 2"));
    assert!(!error.to_string().contains("stale config must not execute"));

    std::fs::write(
      &path,
      "--[[ config header\nmultiline comment ]]\nopt.config_version = 1 -- current API",
    )
    .expect("write versioned config");
    let config = load_file_with_context(&path, &[], LaunchState::default())
      .unwrap_or_else(|error| panic!("supported config API version should load: {error}"));
    assert_eq!(config.config_api_version, Some(CONFIG_API_VERSION));

    std::fs::remove_dir_all(directory).expect("remove temporary config directory");
  }

  fn snapshot() -> AppSnapshot {
    AppSnapshot {
      session: None,
      last_error: None,
      tabs: vec![
        TabSnapshot {
          id: 10,
          name: "1".to_owned(),
          index: 1,
          title: "shell-one".to_owned(),
          selected: false,
          exit_code: None,
        },
        TabSnapshot {
          id: 20,
          name: "2".to_owned(),
          index: 2,
          title: "shell-two".to_owned(),
          selected: true,
          exit_code: None,
        },
      ],
      active_tab: Some(1),
      leader_mode: true,
      terminal_rows: 40,
      terminal_columns: 120,
    }
  }

  // Developer approved: do not remove.
  #[test]
  fn pre_client_handlers_receive_arguments_and_mutate_launch_state() {
    let launch = LaunchState {
      command: "list".to_owned(),
      session_name: None,
      create_if_missing: false,
      fail_if_exists: false,
      require_session: true,
      working_directory: "/work".into(),
    };
    let config = load_config_with_context(
      r#"
            assert(args[1] == "list")
            assert(args[2] == "-a")
            cmd.event.on("PreClient", function(event)
                assert(event.name == "PreClient")
                launch.session = "alpha"
                launch.working_directory = "/workspace/alpha"
                launch.create_if_missing = true
                cmd.tab.create()
            end, { interrupt = true })
            cmd.event.on("ClientLoaded", function(event)
                assert(event.name == "ClientLoaded")
            end)
            "#,
      &["list".to_owned(), "-a".to_owned()],
      launch,
    )
    .expect("startup Lua configuration");

    let handlers = config.lua.event_handlers("PreClient");
    assert_eq!(handlers.len(), 1);
    assert!(handlers[0].interrupt);
    assert_eq!(
      config
        .lua
        .invoke_event_handler(&handlers[0], &AppEvent {
          name: "PreClient".to_owned(),
          ..AppEvent::default()
        })
        .expect("run PreClient handler"),
      vec![RuntimeCommand::NewTab]
    );
    let launch = config.lua.launch_state().expect("launch state");
    assert_eq!(launch.session_name.as_deref(), Some("alpha"));
    assert_eq!(launch.working_directory, PathBuf::from("/workspace/alpha"));
    assert!(launch.create_if_missing);

    let handlers = config.lua.event_handlers("ClientLoaded");
    assert_eq!(handlers.len(), 1);
    assert!(!handlers[0].interrupt);
    config
      .lua
      .invoke_event_handler(&handlers[0], &AppEvent {
        name: "ClientLoaded".to_owned(),
        ..AppEvent::default()
      })
      .expect("run ClientLoaded handler");
  }

  #[test]
  fn event_handlers_can_listen_to_multiple_events() {
    let config = load_config(
      r#"
        cmd.event.on({"session_created", "session_closed"}, function(event)
          assert(event.name == "session_created" or event.name == "session_closed")
        end, { interrupt = true })
      "#,
    )
    .expect("multi-event handler config");

    for name in ["session_created", "session_closed"] {
      let handlers = config.lua.event_handlers(name);
      assert_eq!(handlers.len(), 1);
      assert!(handlers[0].interrupt);
      config
        .lua
        .invoke_event_handler(&handlers[0], &AppEvent {
          name: name.to_owned(),
          ..AppEvent::default()
        })
        .expect("run multi-event handler");
    }
  }

  // Developer approved: do not remove.
  #[test]
  fn sample_configuration_loads_with_luajit() {
    let config = load_config(DEFAULT_CONFIG_SOURCE).expect("built-in Lua config");
    assert_eq!(config.config_api_version, Some(CONFIG_API_VERSION));
    assert_eq!(config.prefix_key, normalize_prefix_key("<C-Space>").expect("prefix"));
    assert_eq!(config.sequence_timeout, Duration::from_millis(700));
    config.lua.sync_state(&snapshot()).expect("sync UI state");
    let segments = config
      .lua
      .render_bar_part("top", "left")
      .expect("sample top bar")
      .expect("sample config supplies top-left tabs");
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[1].text, " 2 shell-two ");
    assert_eq!(segments[1].style.and_then(|style| style.bg), Some(Color::Cyan));
  }

  // Developer approved: do not remove.
  #[test]
  fn repeated_ui_state_and_bar_callbacks_do_not_exhaust_the_lua_stack() {
    let config = load_config(DEFAULT_CONFIG_SOURCE).expect("built-in Lua config");
    let mut state = snapshot();
    state.session = Some(zmux::protocol::SessionSnapshot {
      number: "9".to_owned(),
      active_workspace_id: 4,
      workspaces: vec![zmux::protocol::WorkspaceSnapshot {
        id: 4,
        name: "project".to_owned(),
        explicit_name: None,
        working_directory: "/work/project".into(),
        tabs: vec![zmux::protocol::TabSnapshot {
          id: 20,
          name: "2".to_owned(),
          title: "shell-two".to_owned(),
          exit_code: None,
        }],
        active_tab_id: 20,
      }],
    });
    config
      .lua
      .globals
      .set("temporary", true)
      .expect("add a temporary UI-state field");
    config.lua.sync_state(&state).expect("initial state sync");
    assert!(matches!(
      config
        .lua
        .globals
        .get::<Value>("temporary")
        .expect("read temporary field"),
      Value::Nil
    ));

    for frame in 0..700 {
      config
        .lua
        .sync_state(&state)
        .unwrap_or_else(|error| panic!("frame {frame}: sync state failed: {error:?}"));
      for anchor in ["top", "bottom"] {
        for part in ["left", "center", "right"] {
          config
            .lua
            .render_bar_part(anchor, part)
            .unwrap_or_else(|error| panic!("frame {frame}: {anchor}.{part} failed: {error:?}"));
        }
      }
      let mut fallback = BarSegment {
        text: "fallback".to_owned(),
        role: Some("tab".to_owned()),
        style: None,
      };
      config
        .lua
        .apply_default_style(&mut fallback)
        .unwrap_or_else(|error| panic!("frame {frame}: fallback style failed: {error:?}"));
    }
  }

  // Developer approved: do not remove.
  #[test]
  fn which_key_options_accept_rows_and_percentages_with_a_sixty_percent_cap() {
    let config = load_config(
      r#"
            opt.ui.which.maxcolumns = 4
            opt.ui.which.maxheight = "90%"
            "#,
    )
    .expect("which-key configuration");
    let options = config.lua.sync_options().expect("UI options");
    assert_eq!(options.which.max_columns, 4);
    assert_eq!(options.which.max_height.rows(100), 60);

    let config = load_config(
      r#"
            opt.ui.which.maxheight = 20
            "#,
    )
    .expect("which-key row configuration");
    let options = config.lua.sync_options().expect("UI options");
    assert_eq!(options.which.max_height.rows(100), 20);
  }

  #[test]
  fn pty_border_options_accept_presets_styles_and_none() {
    let config = load_config(
      r#"
        opt.ui.pty.border = "rounded"
        opt.ui.pty.border_style = { fg = "cyan", bold = true }
      "#,
    )
    .expect("PTY border configuration");
    let options = config.lua.sync_options().expect("UI options");
    assert_eq!(options.pty.border, DrawerBorder::Rounded);
    assert_eq!(options.pty.border_style.fg, Some(Color::Cyan));
    assert!(
      options
        .pty
        .border_style
        .add_modifier
        .contains(Modifier::BOLD)
    );

    let config = load_config("opt.ui.pty.border = 'none'").expect("borderless PTY config");
    let options = config.lua.sync_options().expect("UI options");
    assert_eq!(options.pty.border, DrawerBorder::None);

    let config = load_config("opt.ui.pty.border = 'zigzag'").expect("load invalid border value");
    assert!(config.lua.sync_options().is_err());
  }

  // Developer approved: do not remove.
  #[test]
  fn generic_picker_returns_selected_items_and_nil_on_escape_to_lua_callback() {
    let config = load_config(
      r#"
            cmd.event.on("tab_selected", function()
                cmd.ui.pick({"alpha", {label = "beta", value = 42}}, function(item)
                    if item == nil then
                        cmd.tab.previous()
                    elseif item == 42 then
                        cmd.tab.select(2)
                    end
                end)
            end)
            "#,
    )
    .expect("picker callback config");
    let callbacks = config.lua.event_handlers("tab_selected");
    let commands = config
      .lua
      .invoke_event_handler(&callbacks[0], &AppEvent::default())
      .expect("request picker");
    let RuntimeCommand::OpenPicker(picker_id) = commands[0] else {
      panic!("cmd.ui.pick should request a generic picker");
    };
    assert_eq!(config.lua.picker_labels(picker_id), Some(vec!["alpha".into(), "beta".into()]));
    assert_eq!(
      config
        .lua
        .complete_picker(picker_id, Some(1))
        .expect("complete selected item"),
      vec![RuntimeCommand::SelectTab(2)]
    );

    let callbacks = config.lua.event_handlers("tab_selected");
    let commands = config
      .lua
      .invoke_event_handler(&callbacks[0], &AppEvent::default())
      .expect("request second picker");
    let RuntimeCommand::OpenPicker(picker_id) = commands[0] else {
      panic!("cmd.ui.pick should request a generic picker");
    };
    assert_eq!(
      config
        .lua
        .complete_picker(picker_id, None)
        .expect("cancel picker"),
      vec![RuntimeCommand::PreviousTab]
    );
  }

  // Developer approved: do not remove.
  #[test]
  fn lua_receives_server_authoritative_session_and_workspace_state() {
    let config = load_config(
            "cmd.event.on('tab_selected', function()\n  assert(g.session.number == '9')\n  assert(g.active_workspace.name == 'project')\n  assert(g.active_workspace.tabs[1].selected)\n  assert(g.active_workspace.tabs[1].name == '2')\n  cmd.tab.next()\nend)",
        )
        .expect("session-aware callback config");
    let mut state = snapshot();
    state.session = Some(zmux::protocol::SessionSnapshot {
      number: "9".to_owned(),
      active_workspace_id: 4,
      workspaces: vec![zmux::protocol::WorkspaceSnapshot {
        id: 4,
        name: "project".to_owned(),
        explicit_name: None,
        working_directory: "/work/project".into(),
        tabs: vec![zmux::protocol::TabSnapshot {
          id: 20,
          name: "2".to_owned(),
          title: "shell-two".to_owned(),
          exit_code: None,
        }],
        active_tab_id: 20,
      }],
    });
    config.lua.sync_state(&state).expect("sync Session state");
    let callbacks = config.lua.event_handlers("tab_selected");
    let commands = config
      .lua
      .invoke_event_handler(&callbacks[0], &AppEvent::default())
      .expect("invoke Session-aware callback");

    assert_eq!(commands, vec![RuntimeCommand::NextTab]);
  }

  // Developer approved: do not remove.
  #[test]
  fn callbacks_can_be_bound_unbound_and_invoked() {
    let config = load_config(
            "cmd.key.bind('tw', function() cmd.tab.close() end, { desc = 'Close focused tab' })\ncmd.key.bind('x', function() cmd.tab.next() end)\ncmd.key.bind('x', nil)",
        )
        .expect("callback bindings");
    assert_eq!(config.keymap.binding_count(), 1);
    let callback = config
      .keymap
      .binding_for(&parse_key_notation("tw").expect("sequence"))
      .expect("bound callback");
    assert_eq!(config.lua.invoke_key_callback(&callback).expect("invoke"), vec![
      RuntimeCommand::CloseTab
    ]);
    assert!(
      config
        .keymap
        .binding_for(&parse_key_notation("x").expect("key"))
        .is_none()
    );
    assert!(load_config("cmd.key.bind('q', 'quit')").is_err());
    assert!(load_config("cmd.key.bind('<Esc>', function() end)").is_err());
    assert!(load_config("cmd.key.bind('a<Esc>', function() end)").is_err());
  }

  // Developer approved: do not remove.
  #[test]
  fn lua_remains_usable_after_a_key_callback_errors() {
    let config = load_config(
      "cmd.key.bind('x', function() error('expected callback failure') end)\ncmd.key.bind('y', function() cmd.tab.close() end)",
    )
    .expect("callback bindings");
    let failed = config
      .keymap
      .binding_for(&parse_key_notation("x").expect("key"))
      .expect("failing callback");
    let successful = config
      .keymap
      .binding_for(&parse_key_notation("y").expect("key"))
      .expect("successful callback");

    assert!(config.lua.invoke_key_callback(&failed).is_err());
    assert_eq!(
      config
        .lua
        .invoke_key_callback(&successful)
        .expect("invoke later callback"),
      vec![RuntimeCommand::CloseTab]
    );
  }

  // Developer approved: do not remove.
  #[test]
  fn configured_keybinds_list_effective_sequences_and_descriptions_in_order() {
    let config = load_config(
      "cmd.key.bind('z', function() end, { desc = 'Last' })\ncmd.key.bind('tw', function() end, { desc = 'Close tab' })\ncmd.key.bind('a', function() end)\ncmd.key.bind('z', nil)\ncmd.key.bind('z', function() end, { desc = 'Replaced' })",
    )
    .expect("valid keybind config");

    assert_eq!(config.keymap.entries(), vec![
      ("a".to_owned(), None),
      ("tw".to_owned(), Some("Close tab".to_owned())),
      ("z".to_owned(), Some("Replaced".to_owned())),
    ]);
  }

  // Developer approved: do not remove.
  #[test]
  fn key_options_use_neovim_notation_and_validate_timeout() {
    let config = load_config("").expect("valid defaults");
    assert_eq!(config.cursor_shape, CursorShape::Block);
    assert_eq!(config.default_shell, None);
    assert_eq!(config.prefix_key, normalize_prefix_key("<C-Space>").expect("prefix"));
    assert_eq!(config.sequence_timeout, Duration::from_millis(700));
    assert_eq!(
      load_config("opt.default_shell = '/usr/bin/fish'")
        .expect("configured default shell")
        .default_shell,
      Some(std::path::PathBuf::from("/usr/bin/fish"))
    );
    assert!(load_config("opt.cursor_shape = 'circle'").is_err());
    assert_eq!(
      load_config("opt.sequence_timeout_ms = 0")
        .expect("infinite sequence timeout is supported")
        .sequence_timeout,
      Duration::ZERO
    );
    assert!(load_config("opt.prefix_key = 'ctrl-space'").is_err());
    assert!(load_config("opt.prefix_key = 'tw'").is_err());
  }

  // Developer approved: do not remove.
  #[test]
  fn runtime_commands_cannot_be_called_during_config_loading() {
    assert!(load_config("cmd.tab.next()").is_err());
  }

  #[test]
  fn runtime_callbacks_can_construct_and_render_ui_elements() {
    use ratatui::{
      buffer::Buffer,
      layout::Rect,
    };

    let config = load_config(
      r#"
        local dialog
        cmd.event.on("tab_selected", function()
          if dialog then
            collectgarbage("collect")
            collectgarbage("collect")
          else
            local text = cmd.ui.text({ text = "Runtime" })
            local row = cmd.ui.row({ text })
            local content = cmd.ui.column({ row })
            dialog = cmd.ui.create({ type = "modal", content = content, visible = true })
            dialog.set("keys", { x = function(element) element.hide() end })
          end
        end)
      "#,
    )
    .expect("config with runtime UI callback");
    let callback = config.lua.event_handlers("tab_selected").remove(0);
    let commands = config
      .lua
      .invoke_event_handler(&callback, &AppEvent::default())
      .expect("construct UI in runtime callback");
    config
      .lua
      .invoke_event_handler(&callback, &AppEvent::default())
      .expect("collect unreferenced child handles while the Element owns them");
    assert!(
      commands
        .iter()
        .any(|command| *command == RuntimeCommand::Redraw)
    );
    assert!(config.lua.focused_ui_captures_input());
    let key = parse_key_notation("x").expect("direct UI key")[0];
    assert!(config.lua.focused_ui_binding(key).is_some());

    let area = Rect::new(0, 0, 40, 12);
    let mut buffer = Buffer::empty(area);
    config.lua.render_ui_elements(area, &mut buffer);
    assert!(buffer.content.iter().any(|cell| cell.symbol() == "R"));
  }

  #[test]
  fn ui_constructors_are_rejected_from_bar_render_providers() {
    let config = load_config(
      r#"
        opt.ui.bars.top.left = function()
          cmd.ui.text({ text = "not during rendering" })
          return "bar"
        end
      "#,
    )
    .expect("config with a render-time constructor attempt");

    assert!(config.lua.render_bar_part("top", "left").is_err());
  }

  #[test]
  fn unreferenced_runtime_ui_elements_are_reclaimed_after_lua_collection() {
    use ratatui::{
      buffer::Buffer,
      layout::Rect,
    };

    let config = load_config(
      r#"
        local saved_text
        local collection_requested = false
        local reused_dialog
        cmd.event.on("tab_selected", function()
          if saved_text == nil then
            saved_text = cmd.ui.text({ text = "Kept" })
            local content = cmd.ui.column({ saved_text })
            cmd.ui.create({ type = "modal", content = content, visible = true })
          elseif not collection_requested then
            collectgarbage("collect")
            collectgarbage("collect")
            collection_requested = true
          else
            saved_text.set("text", "Reused")
            reused_dialog = cmd.ui.create({ type = "modal", content = saved_text, visible = true })
          end
        end)
      "#,
    )
    .expect("config with transient runtime UI");
    let callback = config.lua.event_handlers("tab_selected").remove(0);
    config
      .lua
      .invoke_event_handler(&callback, &AppEvent::default())
      .expect("create transient UI around a retained text node");
    config
      .lua
      .invoke_event_handler(&callback, &AppEvent::default())
      .expect("collect an Element after its handle becomes unreachable");
    assert_eq!(config.lua.focused_ui(), None);

    let area = Rect::new(0, 0, 40, 12);
    let mut buffer = Buffer::empty(area);
    config.lua.render_ui_elements(area, &mut buffer);
    assert!(!buffer.content.iter().any(|cell| cell.symbol() == "K"));

    config
      .lua
      .invoke_event_handler(&callback, &AppEvent::default())
      .expect("reuse a still-referenced node after its element is collected");
    assert!(config.lua.focused_ui().is_some());
    let mut buffer = Buffer::empty(area);
    config.lua.render_ui_elements(area, &mut buffer);
    assert!(buffer.content.iter().any(|cell| cell.symbol() == "R"));
  }

  #[test]
  fn ui_create_renders_modal_popover_bottom_panel_and_toast_elements() {
    use ratatui::{
      buffer::Buffer,
      layout::Rect,
    };

    let cases = [
      ("modal", "M", "width = 10, height = 5, border = 'none'", (15, 3)),
      ("popover", "P", "x = 2, y = 1, width = 8, height = 3, border = 'none'", (2, 1)),
      ("bottom_panel", "B", "height = 3, border = 'none'", (0, 9)),
      ("toast", "T", "duration = 5000, position = 'top_right', border = 'none'", (39, 0)),
    ];

    for (element_type, text, options, expected) in cases {
      let source = format!(
        "local content = cmd.ui.text({{ text = '{text}' }})\nlocal element = cmd.ui.create({{ type = '{element_type}', {options}, content = content }})\nelement.show()\nkeep_element = element"
      );
      let config =
        load_config(&source).unwrap_or_else(|error| panic!("create {element_type}: {error}"));
      let area = Rect::new(0, 0, 40, 12);
      let mut buffer = Buffer::empty(area);
      config.lua.render_ui_elements(area, &mut buffer);
      let rendered_at = buffer
        .content
        .iter()
        .position(|cell| cell.symbol() == text)
        .unwrap_or_else(|| panic!("{element_type} did not render content"));
      assert_eq!((rendered_at % 40, rendered_at / 40), expected, "{element_type}");
    }

    assert!(load_config(
      "local content = cmd.ui.text({ text = 'x' })\ncmd.ui.create({ type = 'unknown', content = content })"
    )
    .is_err());
  }

  #[test]
  fn modal_and_popover_clamp_to_small_nonzero_origin_areas() {
    use ratatui::{
      buffer::Buffer,
      layout::Rect,
    };

    for specification in [
      "type = 'modal', width = 50, height = 30",
      "type = 'popover', x = 500, y = 300, width = 50, height = 30",
    ] {
      let source = format!(
        "local content = cmd.ui.text({{ text = 'X' }})\nlocal element = cmd.ui.create({{ {specification}, border = 'none', visible = true, content = content }})\nkeep_element = element"
      );
      let config = load_config(&source).expect("small-area overlay config");
      assert!(config.lua.focused_ui().is_some());
      let area = Rect::new(5, 7, 4, 2);
      let mut buffer = Buffer::empty(area);
      config.lua.render_ui_elements(area, &mut buffer);
      assert_eq!(buffer[(5, 7)].symbol(), "X", "{specification}");
    }
  }

  #[test]
  fn modal_dismissal_and_toast_expiration_update_retained_visibility() {
    use ratatui::{
      buffer::Buffer,
      layout::Rect,
    };

    let config = load_config(
      r#"
        local content = cmd.ui.text({ text = "Overlay" })
        local modal = cmd.ui.create({ type = "modal", content = content })
        modal.show()
        local toast_content = cmd.ui.text({ text = "Transient" })
        local toast = cmd.ui.create({ type = "toast", duration = 10, content = toast_content })
        toast.show()
        keep_elements = { modal, toast }
      "#,
    )
    .expect("modal and toast elements");
    assert!(config.lua.focused_ui().is_some());
    assert!(config.lua.focused_ui_captures_input());

    let area = Rect::new(0, 0, 40, 12);
    let mut buffer = Buffer::empty(area);
    config.lua.render_ui_elements(area, &mut buffer);
    assert!(buffer.content.iter().any(|cell| cell.symbol() == "O"));
    assert!(buffer.content.iter().any(|cell| cell.symbol() == "T"));

    assert!(config.lua.dismiss_focused_ui());
    assert!(config.lua.focused_ui().is_none());
    assert!(
      config
        .lua
        .expire_ui_toasts(std::time::Instant::now() + Duration::from_secs(1))
    );
    assert!(
      !config
        .lua
        .expire_ui_toasts(std::time::Instant::now() + Duration::from_secs(2))
    );

    let mut buffer = Buffer::empty(area);
    config.lua.render_ui_elements(area, &mut buffer);
    assert!(!buffer.content.iter().any(|cell| cell.symbol() == "O"));
    assert!(!buffer.content.iter().any(|cell| cell.symbol() == "T"));
  }

  #[test]
  fn dismissing_modal_restores_the_previously_focused_drawer() {
    let config = load_config(
      r#"
        local drawer_content = cmd.ui.text({ text = "Drawer" })
        local drawer = cmd.ui.create({ type = "drawer", content = drawer_content })
        drawer.show()
        drawer.focus()
        local modal_content = cmd.ui.text({ text = "Dialog" })
        local modal = cmd.ui.create({ type = "modal", content = modal_content })
        cmd.event.on("ClientLoaded", function()
          drawer.focus()
          modal.show()
        end)
      "#,
    )
    .expect("nested focus config");
    let drawer_id = config.lua.focused_ui().expect("focused Drawer");
    let callbacks = config.lua.event_handlers("ClientLoaded");
    config
      .lua
      .invoke_event_handler(&callbacks[0], &AppEvent::default())
      .expect("show modal over Drawer");
    assert_ne!(config.lua.focused_ui(), Some(drawer_id));
    assert!(config.lua.dismiss_focused_ui());
    assert_eq!(config.lua.focused_ui(), Some(drawer_id));
  }

  #[test]
  fn drawers_accept_ratatui_border_presets_and_reject_unknown_styles() {
    for border in [
      "none",
      "plain",
      "rounded",
      "double",
      "thick",
      "light_double_dashed",
      "heavy_double_dashed",
      "light_triple_dashed",
      "heavy_triple_dashed",
      "light_quadruple_dashed",
      "heavy_quadruple_dashed",
      "quadrant_inside",
      "quadrant_outside",
    ] {
      let source = format!(
        "local text = cmd.ui.text({{text = 'x'}})\ncmd.ui.create({{content = text, border = '{border}'}})"
      );
      load_config(&source).unwrap_or_else(|error| panic!("border preset {border}: {error}"));
    }
    assert!(
      load_config(
        "local text = cmd.ui.text({text = 'x'})\ncmd.ui.create({content = text, border = 'zigzag'})"
      )
      .is_err()
    );
  }

  #[test]
  fn retained_layouts_reject_ancestor_cycles_and_multiple_parents() {
    let cycle = load_config(
      r#"
        local child = cmd.ui.column({})
        local root = cmd.ui.column({ child })
        child.set("children", { root })
      "#,
    );
    assert!(cycle.is_err());

    let multiple_parents = load_config(
      r#"
        local node = cmd.ui.text({ text = "shared" })
        cmd.ui.column({ node })
        cmd.ui.row({ node })
      "#,
    );
    assert!(multiple_parents.is_err());
  }

  #[test]
  fn detached_child_handles_remain_mutable_and_can_be_reattached() {
    use ratatui::{
      buffer::Buffer,
      layout::Rect,
    };

    let config = load_config(
      r#"
        local ui = cmd.ui
        local detached = ui.text({ text = "Before" })
        local replacement = ui.text({ text = "Temporary" })
        local content = ui.column({ detached })
        local drawer = ui.create({ content = content, visible = true })
        cmd.event.on("ClientLoaded", function()
          content.set("children", { replacement })
          detached.set("text", "After detach")
          content.set("children", { detached })
        end)
      "#,
    )
    .expect("valid retained tree configuration");
    let callbacks = config.lua.event_handlers("ClientLoaded");
    let commands = config
      .lua
      .invoke_event_handler(&callbacks[0], &AppEvent::default())
      .expect("mutate and reattach detached node");
    assert_eq!(commands, vec![
      RuntimeCommand::Redraw,
      RuntimeCommand::Redraw,
      RuntimeCommand::Redraw,
    ]);

    let area = Rect::new(0, 0, 40, 10);
    let mut buffer = Buffer::empty(area);
    config.lua.render_ui_elements(area, &mut buffer);
    let contents = buffer
      .content
      .iter()
      .map(|cell| cell.symbol())
      .collect::<String>();
    assert!(contents.contains("After detach"));
    assert!(!contents.contains("Temporary"));
  }

  #[test]
  fn retained_drawer_handles_mutate_nested_content_and_invoke_direct_keys() {
    use crossterm::event::{
      KeyCode,
      KeyEvent,
      KeyModifiers,
    };
    use ratatui::{
      buffer::Buffer,
      layout::Rect,
    };

    let config = load_config(
      r#"
        local ui = cmd.ui
        local status = ui.text({ text = "Idle" })
        local content = ui.column({ status })
        local drawer = ui.create({
          type = "drawer",
          side = "right",
          width = 16,
          title = "Tasks",
          border = "rounded",
          content = content,
          keys = {
            x = function(element, key)
              assert(key.key == "x")
              status.set("text", "Ready")
              element.set("title", "Updated")
            end,
          },
        })
        drawer.show()
        drawer.focus()
      "#,
    )
    .expect("valid retained UI configuration");
    let key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
    let stroke = KeyStroke::from_crossterm(key).expect("convert direct key");
    let (_, callback, handle) = config
      .lua
      .focused_ui_binding(stroke)
      .expect("focused Drawer binding");
    assert_eq!(
      config
        .lua
        .invoke_ui_key_callback(&callback, handle, key)
        .expect("invoke Drawer binding"),
      vec![RuntimeCommand::Redraw, RuntimeCommand::Redraw]
    );

    let area = Rect::new(0, 0, 40, 10);
    let mut buffer = Buffer::empty(area);
    config.lua.render_ui_elements(area, &mut buffer);
    let contents = buffer
      .content
      .iter()
      .map(|cell| cell.symbol())
      .collect::<String>();
    assert!(contents.contains("Updated"));
    assert!(contents.contains("Ready"));
    assert_eq!(
      buffer[(25, 1)].symbol(),
      "R",
      "fixed-width right drawer content starts inside its border"
    );
  }

  // Developer approved: do not remove.
  #[test]
  fn bar_functions_receive_synced_global_state_and_apply_styles() {
    let config = load_config(
      r#"
            opt.ui.anchors.top.height = 2
            opt.ui.anchors.bottom.visible = false
            opt.ui.bars.top.left = function(g)
                return {{
                    text = g.active_tab.title,
                    role = "left",
                    style = function(segment, current)
                        return {fg = current.active_tab.selected and "green" or "red", bold = true}
                    end,
                }}
            end
            "#,
    )
    .expect("valid UI configuration");
    config.lua.sync_state(&snapshot()).expect("state sync");
    let options = config.lua.sync_options().expect("UI options");
    assert_eq!(options.top.height, 2);
    assert!(!options.bottom.visible);
    let segments = config
      .lua
      .render_bar_part("top", "left")
      .expect("render callback")
      .expect("configured bar part");
    assert_eq!(segments[0].text, "shell-two");
    assert_eq!(segments[0].style.and_then(|style| style.fg), Some(Color::Green));
  }
}
