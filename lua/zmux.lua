---@meta

---@alias ZmuxColor string|integer

---@class ZmuxStyle
---@field fg? ZmuxColor Foreground color name, palette index, or RGB hex string.
---@field bg? ZmuxColor Background color name, palette index, or RGB hex string.
---@field bold? boolean
---@field italic? boolean
---@field underline? boolean
---@field reverse? boolean

---@class ZmuxTerminalState
---@field rows integer Current terminal row count.
---@field columns integer Current terminal column count.

---@class ZmuxTab
---@field id integer Stable tab identifier for this application run.
---@field index integer One-based tab number.
---@field title string Display title for the tab.
---@field selected boolean Whether this tab is currently selected.
---@field running boolean Whether the tab's shell process is still running.
---@field exit_code? integer Exit code when the process has exited.

---@class ZmuxWorkspace
---@field id integer Stable workspace identifier.
---@field index integer One-based workspace position in the Session.
---@field name string Explicit name or working-directory basename.
---@field explicit_name? string Explicit Workspace name, nil when derived from the directory.
---@field working_directory string Starting directory for new tabs in this Workspace.
---@field selected boolean Whether this Workspace is active in the Session.
---@field tabs ZmuxTab[] Tabs in this Workspace.
---@field active_tab ZmuxTab|nil Last active Tab in this Workspace.

---@class ZmuxSession
---@field number integer Automatically assigned Session Number.
---@field active_workspace_id integer Identifier of the active Workspace.
---@field workspaces ZmuxWorkspace[] Workspaces in creation order.
---@field active_workspace ZmuxWorkspace|nil Currently active Workspace.

---@class ZmuxGlobalState
---@field session ZmuxSession|nil Current server-owned Session.
---@field workspaces ZmuxWorkspace[] Workspaces in the current Session.
---@field active_workspace ZmuxWorkspace|nil Currently active Workspace.
---@field tabs ZmuxTab[] Tabs in the active Workspace, in display order.
---@field active_tab ZmuxTab|nil Currently selected tab.
---@field last_error? string Most recent server command error, if any.
---@field leader_mode boolean Whether the prefix/leader key mode is active.
---@field terminal ZmuxTerminalState Current outer terminal dimensions.

---@alias ZmuxEventName
---| "session_created"
---| "session_selected"
---| "session_closed"
---| "workspace_created"
---| "workspace_closed"
---| "workspace_selected"
---| "tab_created"
---| "tab_closed"
---| "tab_selected"
---| "leader_mode_changed"
---| "terminal_title_changed"
---| "process_exited"
---| "terminal_resized"

---@class ZmuxEvent
---@field name ZmuxEventName Event name.
---@field session_number? integer Affected Session Number, when applicable.
---@field workspace_id? integer Affected Workspace identifier, when applicable.
---@field workspace_index? integer Current one-based Workspace position, when applicable.
---@field previous_workspace_id? integer Previously active Workspace identifier.
---@field tab_id? integer Affected tab identifier, when applicable.
---@field index? integer Current one-based tab number, when applicable.
---@field previous_index? integer Previously selected one-based tab number.
---@field title? string New terminal title, when applicable.
---@field exit_code? integer Process exit code, when available.
---@field rows? integer New terminal row count.
---@field columns? integer New terminal column count.
---@field active? boolean New leader-mode state, when applicable.

---@class ZmuxKeymapOptions
---@field desc? string Short description shown in the key-sequence popup.

---@class ZmuxEventApi
---@field on fun(name: ZmuxEventName, callback: fun(event: ZmuxEvent)) Register a callback for an application event.

---@alias ZmuxPickerItem string|{label?: string, text?: string, value?: any}

---@class ZmuxCommandApi: ZmuxEventApi
---@field bind fun(keys: string, callback: fun()|nil, opts?: ZmuxKeymapOptions) Bind a neovim-notation key sequence after the configured prefix; pass nil to unbind.
---@field pick fun(items: ZmuxPickerItem[], callback: fun(item: any|nil)) Open a generic fuzzy picker from a key or event callback and later call back with the chosen item or nil on Escape.
---@field quit fun() Detach the Client while preserving its Session. Available from key and event callbacks.
---@field detach fun() Detach the Client while preserving its Session.
---@field kill_session fun() Kill the current Session from a key or event callback.
---@field stop_server fun() Stop the Server and all Sessions from a key or event callback.
---@field toggle_workspace_drawer fun() Expand or collapse the local Workspace Drawer from a key or event callback.
---@field create_workspace fun() Open the Workspace creation dialog from a key or event callback.
---@field edit_workspace fun() Open the active Workspace editor from a key or event callback.
---@field new_tab fun() Request creation of a terminal tab from a key or event callback.
---@field close_tab fun() Request closure of the active tab from a key or event callback.
---@field next_tab fun() Request selection of the next tab from a key or event callback.
---@field previous_tab fun() Request selection of the previous tab from a key or event callback.
---@field select_tab fun(index: integer) Request selection of a one-based tab index from a key or event callback.

---@class ZmuxBarSegment
---@field text string Text displayed by this segment.
---@field role? string Semantic role used by the default style function.
---@field style? ZmuxStyle|fun(segment: ZmuxBarSegment, g: ZmuxGlobalState): ZmuxStyle Static or state-dependent style override.

---@alias ZmuxBarContent string|ZmuxBarSegment|ZmuxBarSegment[]
---@alias ZmuxBarContentProvider fun(g: ZmuxGlobalState): ZmuxBarContent
---@alias ZmuxBarPartContent ZmuxBarContent|ZmuxBarContentProvider
---@alias ZmuxDefaultStyleProvider fun(segment: ZmuxBarSegment, g: ZmuxGlobalState): ZmuxStyle

---@class ZmuxAnchorOptions
---@field height? integer Reserved height in terminal rows. Defaults to one row.
---@field visible? boolean Whether this anchor is shown. Defaults to true.

---@class ZmuxBarOptions
---@field left? ZmuxBarPartContent
---@field center? ZmuxBarPartContent
---@field right? ZmuxBarPartContent

---@class ZmuxAnchors
---@field top? ZmuxAnchorOptions
---@field bottom? ZmuxAnchorOptions

---@class ZmuxBars
---@field top? ZmuxBarOptions
---@field bottom? ZmuxBarOptions

---@class ZmuxUIOptions
---@field anchors? ZmuxAnchors
---@field bars? ZmuxBars
---@field style? ZmuxDefaultStyleProvider Default style function, overridden by a segment's explicit style.

---@class ZmuxTerminalSizeOptions
---@field policy? "latest"|"largest"|"smallest"|"manual" Policy for choosing the Session's one shared PTY geometry; defaults to "latest".
---@field manual? ZmuxTerminalState Fixed rows and columns when policy is "manual".

---@class ZmuxOptions
---@field cursor_shape? "block"|"underline"|"bar"
---@field ignore_size? boolean Whether this Client's terminal dimensions are excluded from shared PTY sizing; defaults to false.
---@field terminal_size? ZmuxTerminalSizeOptions Server-wide shared PTY sizing policy and manual dimensions.
---@field prefix_key? string Prefix key in neovim notation. Defaults to <C-Space>.
---@field sequence_timeout_ms? integer Timeout in milliseconds between sequence keys. Defaults to 700; zero disables timeout.
---@field ui? ZmuxUIOptions

---@type ZmuxCommandApi
cmd = {}

---@type ZmuxOptions
opt = {}

---@type ZmuxGlobalState
g = {}

---@type ZmuxGlobalState
global = g

---@class ZmuxInfoApi
---@type ZmuxInfoApi
inf = {}
