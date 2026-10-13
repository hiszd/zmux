use crate::{
    input::KeyMap,
    key_notation::{KeyStroke, normalize_prefix_key, parse_key_notation},
    ui::CursorShape,
};
use mlua::{Function, Lua, Result as LuaResult, Table, Value};
use ratatui::style::{Color, Modifier, Style};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    rc::Rc,
    time::Duration,
};
use zmux::protocol::SessionSnapshot;

pub struct AppConfig {
    pub keymap: KeyMap,
    pub cursor_shape: CursorShape,
    pub prefix_key: KeyStroke,
    pub sequence_timeout: Duration,
    pub lua: LuaConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        load_config(include_str!("../zmux.lua")).expect("built-in Lua defaults are valid")
    }
}

#[derive(Clone, Debug)]
pub struct TabSnapshot {
    pub id: u64,
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
    pub session_number: Option<u64>,
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
    Redraw,
    Quit,
    Detach,
    KillSession,
    StopServer,
    ToggleWorkspaceDrawer,
    CreateWorkspace,
    EditWorkspace,
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
pub struct UiOptions {
    pub top: AnchorOptions,
    pub bottom: AnchorOptions,
}

#[derive(Clone, Debug)]
pub struct BarSegment {
    pub text: String,
    pub role: Option<String>,
    pub style: Option<Style>,
}

pub struct LuaCallback(Function);

struct PendingPicker {
    labels: Vec<String>,
    values: Vec<Value>,
    callback: Function,
}

pub struct LuaConfig {
    lua: Lua,
    options: Table,
    globals: Table,
    snapshot: RefCell<AppSnapshot>,
    handlers: Rc<RefCell<HashMap<String, Vec<Function>>>>,
    commands: Rc<RefCell<VecDeque<RuntimeCommand>>>,
    in_event_callback: Rc<Cell<bool>>,
    pending_pickers: Rc<RefCell<HashMap<u64, PendingPicker>>>,
}

impl LuaConfig {
    pub fn sync_state(&self, snapshot: &AppSnapshot) -> LuaResult<()> {
        *self.snapshot.borrow_mut() = snapshot.clone();
        self.globals.clear()?;
        let tabs = self
            .lua
            .create_table_with_capacity(snapshot.tabs.len(), 0)?;
        let mut tab_values = Vec::with_capacity(snapshot.tabs.len());
        for tab in &snapshot.tabs {
            let value = self.lua.create_table()?;
            value.set("id", tab.id)?;
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
                value.set(
                    "working_directory",
                    workspace.working_directory.to_string_lossy().as_ref(),
                )?;
                value.set("selected", workspace.id == session.active_workspace_id)?;
                let workspace_tabs = self
                    .lua
                    .create_table_with_capacity(workspace.tabs.len(), 0)?;
                let mut tab_values = Vec::with_capacity(workspace.tabs.len());
                for (tab_index, tab) in workspace.tabs.iter().enumerate() {
                    let tab_value = self.lua.create_table()?;
                    tab_value.set("id", tab.id)?;
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
            session_value.set("number", session.number)?;
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
        self.globals.set("session", session_value)?;
        self.globals
            .set("last_error", snapshot.last_error.as_deref())?;
        self.globals.set("leader_mode", snapshot.leader_mode)?;
        self.globals.set("terminal", terminal)?;
        Ok(())
    }

    pub fn event_handlers(&self, name: &str) -> Vec<LuaCallback> {
        self.handlers
            .borrow()
            .get(name)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(LuaCallback)
            .collect()
    }

    pub fn invoke_event_handler(
        &self,
        callback: &LuaCallback,
        event: &AppEvent,
    ) -> LuaResult<Vec<RuntimeCommand>> {
        let value = self.event_value(event)?;
        self.invoke_callback(&callback.0, value)
    }

    pub fn invoke_key_callback(&self, callback: &Function) -> LuaResult<Vec<RuntimeCommand>> {
        self.invoke_callback(callback, ())
    }

    fn invoke_callback<A: mlua::IntoLuaMulti>(
        &self,
        callback: &Function,
        args: A,
    ) -> LuaResult<Vec<RuntimeCommand>> {
        self.in_event_callback.set(true);
        let result = callback.call::<()>(args);
        self.in_event_callback.set(false);
        if result.is_err() {
            self.commands.borrow_mut().clear();
        }
        result?;
        Ok(self.commands.borrow_mut().drain(..).collect())
    }

    pub fn picker_labels(&self, picker_id: u64) -> Option<Vec<String>> {
        self.pending_pickers
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
        self.in_event_callback.set(true);
        let result = picker.callback.call::<()>(value);
        self.in_event_callback.set(false);
        if result.is_err() {
            self.commands.borrow_mut().clear();
        }
        result?;
        Ok(self.commands.borrow_mut().drain(..).collect())
    }

    pub fn sync_options(&self) -> LuaResult<UiOptions> {
        let ui = self.options.get::<Option<Table>>("ui")?;
        let Some(ui) = ui else {
            return Ok(UiOptions::default());
        };
        let anchors = ui.get::<Option<Table>>("anchors")?;
        Ok(UiOptions {
            top: read_anchor(anchors.as_ref(), "top")?,
            bottom: read_anchor(anchors.as_ref(), "bottom")?,
        })
    }

    pub fn render_bar_part(&self, anchor: &str, part: &str) -> LuaResult<Option<Vec<BarSegment>>> {
        self.refresh_state()?;
        let Some(ui) = self.options.get::<Option<Table>>("ui")? else {
            return Ok(None);
        };
        let Some(bars) = ui.get::<Option<Table>>("bars")? else {
            return Ok(None);
        };
        let Some(bar) = bars.get::<Option<Table>>(anchor)? else {
            return Ok(None);
        };
        let content = bar.get::<Value>(part)?;
        if matches!(content, Value::Nil) {
            return Ok(None);
        }
        let content = match content {
            Value::Function(function) => function.call::<Value>(self.globals.clone())?,
            value => value,
        };
        let mut rendered = Vec::new();
        for (mut segment, segment_table) in self.parse_segments(content)? {
            if segment.style.is_none()
                && let Value::Function(function) = ui.get::<Value>("style")?
            {
                self.refresh_state()?;
                let value = function.call::<Value>((segment_table, self.globals.clone()))?;
                segment.style = parse_style(value)?;
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
        value.set("session_number", event.session_number)?;
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
                        let value =
                            function.call::<Value>((table.clone(), self.globals.clone()))?;
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
        }
    }
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
    let lua = Lua::new();
    let keymap = Rc::new(RefCell::new(KeyMap::default()));
    let handlers = Rc::new(RefCell::new(HashMap::<String, Vec<Function>>::new()));
    let commands = Rc::new(RefCell::new(VecDeque::<RuntimeCommand>::new()));
    let in_event_callback = Rc::new(Cell::new(false));
    let pending_pickers = Rc::new(RefCell::new(HashMap::<u64, PendingPicker>::new()));
    let next_picker_id = Rc::new(Cell::new(1_u64));
    let config_loading = Rc::new(Cell::new(true));
    let globals = lua.create_table()?;
    lua.globals().set("g", globals.clone())?;
    lua.globals().set("global", globals.clone())?;
    lua.globals().set("inf", lua.create_table()?)?;

    let bind = {
        let keymap = Rc::clone(&keymap);
        let config_loading = Rc::clone(&config_loading);
        lua.create_function(
            move |_, (key, callback, options): (String, Value, Option<Table>)| {
                if !config_loading.get() {
                    return Err(mlua::Error::external(
                        "cmd.bind is only available while loading configuration",
                    ));
                }
                let sequence = parse_key_notation(&key).map_err(mlua::Error::external)?;
                if matches!(callback, Value::Nil) {
                    keymap.borrow_mut().unbind(&sequence);
                    return Ok(());
                }
                let Value::Function(callback) = callback else {
                    return Err(mlua::Error::external(
                        "cmd.bind callback must be a function or nil",
                    ));
                };
                let desc = options
                    .map(|options| options.get::<Option<String>>("desc"))
                    .transpose()?
                    .flatten();
                keymap
                    .borrow_mut()
                    .bind(sequence, callback, desc)
                    .map_err(mlua::Error::external)
            },
        )?
    };
    let on = {
        let handlers = Rc::clone(&handlers);
        let config_loading = Rc::clone(&config_loading);
        lua.create_function(move |_, (name, callback): (String, Function)| {
            if !config_loading.get() {
                return Err(mlua::Error::external(
                    "cmd.on is only available while loading configuration",
                ));
            }
            if !valid_event_name(&name) {
                return Err(mlua::Error::external(format!("unknown event: {name}")));
            }
            handlers
                .borrow_mut()
                .entry(name)
                .or_default()
                .push(callback);
            Ok(())
        })?
    };
    let cmd = lua.create_table()?;
    cmd.set("bind", bind)?;
    cmd.set("on", on)?;
    {
        let pending_pickers = Rc::clone(&pending_pickers);
        let next_picker_id = Rc::clone(&next_picker_id);
        let commands = Rc::clone(&commands);
        let in_event_callback = Rc::clone(&in_event_callback);
        cmd.set(
            "pick",
            lua.create_function(move |_, (items, callback): (Table, Function)| {
                ensure_command_context(&in_event_callback)?;
                let picker_id = next_picker_id.get();
                next_picker_id.set(
                    picker_id.checked_add(1).ok_or_else(|| {
                        mlua::Error::external("picker identifier space exhausted")
                    })?,
                );
                let mut labels = Vec::new();
                let mut values = Vec::new();
                for item in items.sequence_values::<Value>() {
                    let item = item?;
                    let (label, value) = picker_item(item)?;
                    labels.push(label);
                    values.push(value);
                }
                pending_pickers.borrow_mut().insert(
                    picker_id,
                    PendingPicker {
                        labels,
                        values,
                        callback,
                    },
                );
                commands
                    .borrow_mut()
                    .push_back(RuntimeCommand::OpenPicker(picker_id));
                Ok(())
            })?,
        )?;
    }
    register_command(
        &lua,
        &cmd,
        &commands,
        &in_event_callback,
        "quit",
        RuntimeCommand::Quit,
    )?;
    for (name, command) in [
        ("redraw", RuntimeCommand::Redraw),
        ("detach", RuntimeCommand::Detach),
        ("kill_session", RuntimeCommand::KillSession),
        ("stop_server", RuntimeCommand::StopServer),
        (
            "toggle_workspace_drawer",
            RuntimeCommand::ToggleWorkspaceDrawer,
        ),
        ("create_workspace", RuntimeCommand::CreateWorkspace),
        ("edit_workspace", RuntimeCommand::EditWorkspace),
    ] {
        register_command(&lua, &cmd, &commands, &in_event_callback, name, command)?;
    }
    register_command(
        &lua,
        &cmd,
        &commands,
        &in_event_callback,
        "new_tab",
        RuntimeCommand::NewTab,
    )?;
    register_command(
        &lua,
        &cmd,
        &commands,
        &in_event_callback,
        "close_tab",
        RuntimeCommand::CloseTab,
    )?;
    register_command(
        &lua,
        &cmd,
        &commands,
        &in_event_callback,
        "next_tab",
        RuntimeCommand::NextTab,
    )?;
    register_command(
        &lua,
        &cmd,
        &commands,
        &in_event_callback,
        "previous_tab",
        RuntimeCommand::PreviousTab,
    )?;
    {
        let commands = Rc::clone(&commands);
        let in_event_callback = Rc::clone(&in_event_callback);
        cmd.set(
            "select_tab",
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
    let bars = lua.create_table()?;
    for name in ["top", "bottom"] {
        bars.set(name, lua.create_table()?)?;
    }
    ui.set("bars", bars)?;
    options.set("ui", ui)?;
    lua.globals().set("opt", options.clone())?;
    lua.load(source).exec()?;
    config_loading.set(false);

    let cursor_shape = options
        .get::<String>("cursor_shape")?
        .parse::<CursorShape>()
        .map_err(mlua::Error::external)?;
    let prefix_key = options
        .get::<String>("prefix_key")
        .and_then(|value| normalize_prefix_key(&value).map_err(mlua::Error::external))?;
    let timeout_ms = options.get::<u64>("sequence_timeout_ms")?;
    let keymap = keymap.borrow().clone();
    Ok(AppConfig {
        keymap,
        cursor_shape,
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
        },
    })
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
        "session_created"
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

pub fn load_file(path: &std::path::Path) -> LuaResult<AppConfig> {
    let source = std::fs::read_to_string(path).map_err(mlua::Error::external)?;
    load_config(&source)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> AppSnapshot {
        AppSnapshot {
            session: None,
            last_error: None,
            tabs: vec![
                TabSnapshot {
                    id: 10,
                    index: 1,
                    title: "shell-one".to_owned(),
                    selected: false,
                    exit_code: None,
                },
                TabSnapshot {
                    id: 20,
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

    #[test]
    fn sample_configuration_loads_with_luajit() {
        let config = load_config(include_str!("../zmux.lua")).expect("sample Lua config");
        assert_eq!(
            config.prefix_key,
            normalize_prefix_key("<C-Space>").expect("prefix")
        );
        assert_eq!(config.sequence_timeout, Duration::ZERO);
        assert_eq!(config.keymap.binding_count(), 21);
        let callback = config
            .keymap
            .binding_for(&parse_key_notation("x").expect("key"))
            .expect("close tab binding");
        assert_eq!(
            config
                .lua
                .invoke_key_callback(&callback)
                .expect("key callback"),
            vec![RuntimeCommand::CloseTab]
        );
        config.lua.sync_state(&snapshot()).expect("sync UI state");
        let segments = config
            .lua
            .render_bar_part("top", "left")
            .expect("sample top bar")
            .expect("sample config supplies top-left tabs");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[1].text, " 2 shell-two ");
        assert_eq!(
            segments[1].style.and_then(|style| style.bg),
            Some(Color::Cyan)
        );
    }

    #[test]
    fn generic_picker_returns_selected_items_and_nil_on_escape_to_lua_callback() {
        let config = load_config(
            r#"
            cmd.on("tab_selected", function()
                cmd.pick({"alpha", {label = "beta", value = 42}}, function(item)
                    if item == nil then
                        cmd.previous_tab()
                    elseif item == 42 then
                        cmd.select_tab(2)
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
            panic!("cmd.pick should request a generic picker");
        };
        assert_eq!(
            config.lua.picker_labels(picker_id),
            Some(vec!["alpha".into(), "beta".into()])
        );
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
            panic!("cmd.pick should request a generic picker");
        };
        assert_eq!(
            config
                .lua
                .complete_picker(picker_id, None)
                .expect("cancel picker"),
            vec![RuntimeCommand::PreviousTab]
        );
    }

    #[test]
    fn lua_receives_server_authoritative_session_and_workspace_state() {
        let config = load_config(
            "cmd.on('tab_selected', function()\n  assert(g.session.number == 9)\n  assert(g.active_workspace.name == 'project')\n  assert(g.active_workspace.tabs[1].selected)\n  cmd.next_tab()\nend)",
        )
        .expect("session-aware callback config");
        let mut state = snapshot();
        state.session = Some(zmux::protocol::SessionSnapshot {
            number: 9,
            active_workspace_id: 4,
            workspaces: vec![zmux::protocol::WorkspaceSnapshot {
                id: 4,
                name: "project".to_owned(),
                explicit_name: None,
                working_directory: "/work/project".into(),
                tabs: vec![zmux::protocol::TabSnapshot {
                    id: 20,
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

    #[test]
    fn callbacks_can_be_bound_unbound_and_invoked() {
        let config = load_config(
            "cmd.bind('tw', function() cmd.close_tab() end, { desc = 'Close focused tab' })\ncmd.bind('x', function() cmd.next_tab() end)\ncmd.bind('x', nil)",
        )
        .expect("callback bindings");
        assert_eq!(config.keymap.binding_count(), 1);
        let callback = config
            .keymap
            .binding_for(&parse_key_notation("tw").expect("sequence"))
            .expect("bound callback");
        assert_eq!(
            config.lua.invoke_key_callback(&callback).expect("invoke"),
            vec![RuntimeCommand::CloseTab]
        );
        assert!(
            config
                .keymap
                .binding_for(&parse_key_notation("x").expect("key"))
                .is_none()
        );
        assert!(load_config("cmd.bind('q', 'quit')").is_err());
        assert!(load_config("cmd.bind('<Esc>', function() end)").is_err());
        assert!(load_config("cmd.bind('a<Esc>', function() end)").is_err());
    }

    #[test]
    fn key_options_use_neovim_notation_and_validate_timeout() {
        let config = load_config("").expect("valid defaults");
        assert_eq!(config.cursor_shape, CursorShape::Block);
        assert_eq!(
            config.prefix_key,
            normalize_prefix_key("<C-Space>").expect("prefix")
        );
        assert_eq!(config.sequence_timeout, Duration::from_millis(700));
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

    #[test]
    fn lua_commands_are_deferred_until_registered_event_callback_returns() {
        let config = load_config(
            "cmd.on('tab_selected', function(event)\n  assert(global == g)\n  assert(g.active_tab.index == event.index)\n  cmd.next_tab()\nend)",
        )
        .expect("valid callback config");
        config.lua.sync_state(&snapshot()).expect("state sync");
        let event = AppEvent {
            name: "tab_selected".to_owned(),
            index: Some(2),
            ..AppEvent::default()
        };
        let callbacks = config.lua.event_handlers("tab_selected");
        let commands = config
            .lua
            .invoke_event_handler(&callbacks[0], &event)
            .expect("callback dispatch");
        assert_eq!(commands, vec![RuntimeCommand::NextTab]);
    }

    #[test]
    fn runtime_commands_cannot_be_called_during_config_loading() {
        assert!(load_config("cmd.next_tab()").is_err());
    }

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
        assert_eq!(
            segments[0].style.and_then(|style| style.fg),
            Some(Color::Green)
        );
    }
}
