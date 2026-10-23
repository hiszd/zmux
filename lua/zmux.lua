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
---@field number string Session name; automatically created sessions use numeric names represented as strings.
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
---| "PreClient"
---| "ClientLoaded"
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
---@field session_number? string Affected Session name, when applicable.
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

---@class ZmuxEventOptions
---@field interrupt? boolean For PreClient, wait for built-in interactive work requested by this handler before continuing startup.

---@class ZmuxEventApi
---@field on fun(name: ZmuxEventName, callback: fun(event: ZmuxEvent), opts?: ZmuxEventOptions) Register a callback for an application event.

---@alias ZmuxPickerItem string|{label?: string, text?: string, value?: any}

---@class ZmuxKeyApi
---@field bind fun(keys: string, callback: fun()|nil, opts?: ZmuxKeymapOptions) Bind a neovim-notation key sequence after the configured prefix; pass nil to unbind.

---@alias ZmuxUiSize integer|"auto"|"fill"
---@alias ZmuxElementSize integer|"auto"
---@alias ZmuxDrawerSide "left"|"right"
---@alias ZmuxDrawerWidth ZmuxElementSize
---@alias ZmuxDrawerBorder "none"|"plain"|"rounded"|"double"|"thick"|"light_double_dashed"|"heavy_double_dashed"|"light_triple_dashed"|"heavy_triple_dashed"|"light_quadruple_dashed"|"heavy_quadruple_dashed"|"quadrant_inside"|"quadrant_outside"
---@alias ZmuxToastPosition "top_right"|"top_center"|"bottom_right"|"bottom_center"
---@alias ZmuxUiElementProperty "side"|"width"|"height"|"x"|"y"|"position"|"duration"|"title"|"border"|"border_style"|"keys"
---@alias ZmuxUiElementPropertyValue ZmuxDrawerSide|ZmuxElementSize|integer|string|ZmuxDrawerBorder|ZmuxStyle|table<string, fun(element: ZmuxUiElementHandle, key: ZmuxUiKey)>
---@alias ZmuxDrawerProperty ZmuxUiElementProperty
---@alias ZmuxDrawerPropertyValue ZmuxUiElementPropertyValue
---@alias ZmuxTextProperty "text"|"style"
---@alias ZmuxLayoutProperty "children"

---@class ZmuxUiKey
---@field key string Neovim-notation key that triggered the binding.
---@field ctrl boolean Whether Control was pressed.
---@field alt boolean Whether Alt was pressed.
---@field shift boolean Whether Shift was pressed.
---@field is_repeat boolean Whether this is a repeated key event.

---@class ZmuxUiNodeHandle
---@field set fun(property: ZmuxTextProperty|ZmuxLayoutProperty, value: string|ZmuxStyle|ZmuxLayoutChild[]) Set a typed property; mutations automatically schedule a redraw.

---@class ZmuxTextOptions
---@field text string Initial text; newlines create additional unwrapped lines.
---@field style? ZmuxStyle Initial text style.

---@class ZmuxTextHandle: ZmuxUiNodeHandle
---@field set fun(property: ZmuxTextProperty, value: string|ZmuxStyle) Set text or its style.

---@alias ZmuxLayoutChild ZmuxUiNodeHandle|{node: ZmuxUiNodeHandle, size?: ZmuxUiSize}

---@class ZmuxLayoutHandle: ZmuxUiNodeHandle
---@field set fun(property: ZmuxLayoutProperty, value: ZmuxLayoutChild[]) Replace this Layout's children.

---@class ZmuxDrawerOptions
---@field type? "drawer" Element type; defaults to "drawer".
---@field side? ZmuxDrawerSide Edge to attach to. Defaults to "left".
---@field width? ZmuxDrawerWidth Fixed cell width or content-sized "auto". Defaults to "auto".
---@field title? string Optional border title.
---@field border? ZmuxDrawerBorder Ratatui border preset. Defaults to "plain".
---@field border_style? ZmuxStyle Border color and modifiers.
---@field content ZmuxUiNodeHandle Nested content tree.
---@field keys? table<string, fun(element: ZmuxUiElementHandle, key: ZmuxUiKey)> Direct bindings active while this element is focused.
---@field visible? boolean Initial visibility. Defaults to false.

---@class ZmuxModalOptions
---@field type "modal"
---@field width? ZmuxElementSize Width in cells or "auto"; defaults to content width.
---@field height? ZmuxElementSize Height in cells or "auto"; defaults to content height.
---@field title? string Optional border title.
---@field border? ZmuxDrawerBorder Ratatui border preset. Defaults to "plain".
---@field border_style? ZmuxStyle Border color and modifiers.
---@field content ZmuxUiNodeHandle Nested content tree.
---@field keys? table<string, fun(element: ZmuxUiElementHandle, key: ZmuxUiKey)> Direct bindings; modal input is captured until Escape or hide().
---@field visible? boolean Initial visibility. Defaults to false.

---@class ZmuxPopoverOptions
---@field type "popover"
---@field x? integer Horizontal cell offset in the UI area; defaults to 0 and is clamped to fit.
---@field y? integer Vertical cell offset in the UI area; defaults to 0 and is clamped to fit.
---@field width? ZmuxElementSize Width in cells or "auto"; defaults to content width.
---@field height? ZmuxElementSize Height in cells or "auto"; defaults to content height.
---@field title? string Optional border title.
---@field border? ZmuxDrawerBorder Ratatui border preset. Defaults to "plain".
---@field border_style? ZmuxStyle Border color and modifiers.
---@field content ZmuxUiNodeHandle Nested content tree.
---@field keys? table<string, fun(element: ZmuxUiElementHandle, key: ZmuxUiKey)> Direct bindings; popover input is captured until Escape or hide().
---@field visible? boolean Initial visibility. Defaults to false.

---@class ZmuxBottomPanelOptions
---@field type "bottom_panel"
---@field height? ZmuxElementSize Height in cells or "auto"; defaults to content height.
---@field title? string Optional border title.
---@field border? ZmuxDrawerBorder Ratatui border preset. Defaults to "plain".
---@field border_style? ZmuxStyle Border color and modifiers.
---@field content ZmuxUiNodeHandle Nested content tree.
---@field keys? table<string, fun(element: ZmuxUiElementHandle, key: ZmuxUiKey)> Direct bindings while focused.
---@field visible? boolean Initial visibility. Defaults to false.

---@class ZmuxToastOptions
---@field type "toast"
---@field position? ZmuxToastPosition Screen corner or center edge; defaults to "top_right".
---@field duration? integer Visible duration in milliseconds; defaults to 3000.
---@field title? string Optional border title.
---@field border? ZmuxDrawerBorder Ratatui border preset. Defaults to "plain".
---@field border_style? ZmuxStyle Border color and modifiers.
---@field content ZmuxUiNodeHandle Nested content tree.
---@field visible? boolean Initial visibility. Defaults to false.

---@alias ZmuxUiElementOptions ZmuxDrawerOptions|ZmuxModalOptions|ZmuxPopoverOptions|ZmuxBottomPanelOptions|ZmuxToastOptions

---@class ZmuxUiElementHandle
---@field set fun(property: ZmuxUiElementProperty, value: ZmuxUiElementPropertyValue) Set a typed property; visual changes automatically schedule a redraw.
---@field show fun() Show this element; Modal and Popover are focused automatically.
---@field hide fun() Hide this element and release its focus if needed.
---@field toggle fun() Toggle visibility; showing Modal and Popover focuses them.
---@field focus fun() Focus this element, showing it if necessary. Toasts cannot be focused.
---@field blur fun() Return keyboard focus to the terminal; Modal and Popover elements must be hidden instead.

---@alias ZmuxDrawerHandle ZmuxUiElementHandle

---@class ZmuxUiApi
---@field pick fun(items: ZmuxPickerItem[], callback: fun(item: any|nil)) Open a generic fuzzy picker from a key or event callback and later call back with the chosen item or nil on Escape.
---@field redraw fun() Request a client redraw.
---@field create fun(options: ZmuxUiElementOptions): ZmuxUiElementHandle Create a retained Drawer, Modal, Popover, Bottom Panel, or Toast while loading configuration.
---@field text fun(options: ZmuxTextOptions): ZmuxTextHandle Create a Text node.
---@field row fun(children: ZmuxLayoutChild[]): ZmuxLayoutHandle Create a horizontal Layout.
---@field column fun(children: ZmuxLayoutChild[]): ZmuxLayoutHandle Create a vertical Layout.

---@class ZmuxClientApi
---@field detach fun() Detach the Client while preserving its Session.

---@class ZmuxServerApi
---@field stop fun() Stop the Server and all Sessions.

---@class ZmuxSessionApi
---@field kill fun() Kill the current Session.
---@field switch fun() Open the Session picker.

---@class ZmuxWorkspaceApi
---@field create fun() Open the Workspace creation dialog.
---@field edit fun() Open the active Workspace editor.
---@field close fun() Close the active Workspace and all of its Tabs.
---@field next fun() Select the next Workspace.
---@field previous fun() Select the previous Workspace.
---@field select fun(id: integer) Select a Workspace by stable identifier.

---@class ZmuxTabApi
---@field create fun() Request creation of a terminal Tab.
---@field close fun() Request closure of the active Tab.
---@field next fun() Request selection of the next Tab.
---@field previous fun() Request selection of the previous Tab.
---@field select fun(index: integer) Request selection of a one-based Tab index.

---@class ZmuxLaunchState
---@field command string Original CLI subcommand (informational; changing it does not change dispatch).
---@field session? string Session selected for the Client; set this during PreClient to change the target.
---@field create_if_missing boolean Whether a missing named Session should be created.
---@field fail_if_exists boolean Whether startup should fail when the named Session already exists.
---@field require_session boolean Whether startup requires an existing selected Session.
---@field working_directory string Working directory used when creating a Session.

---@class ZmuxCommandApi
---@field key ZmuxKeyApi
---@field event ZmuxEventApi
---@field ui ZmuxUiApi
---@field client ZmuxClientApi
---@field server ZmuxServerApi
---@field session ZmuxSessionApi
---@field workspace ZmuxWorkspaceApi
---@field tab ZmuxTabApi

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

---@class ZmuxWhichOptions
---@field maxcolumns? integer Maximum number of which-key columns. Defaults to 7.
---@field maxheight? integer|string Maximum dialog height in rows or as a percentage string, clamped to 60% of the screen. Defaults to "55%".

---@class ZmuxUIOptions
---@field anchors? ZmuxAnchors
---@field bars? ZmuxBars
---@field which? ZmuxWhichOptions
---@field style? ZmuxDefaultStyleProvider Default style function, overridden by a segment's explicit style.

---@class ZmuxTerminalSizeOptions
---@field policy? "latest"|"largest"|"smallest"|"manual" Policy for choosing the Session's one shared PTY geometry; defaults to "latest".
---@field manual? ZmuxTerminalState Fixed rows and columns when policy is "manual".

---@class ZmuxOptions
---@field config_version integer Required config API version; must match `zmux.config_api_version`.
---@field cursor_shape? "block"|"underline"|"bar"
---@field default_shell? string Shell executable for newly created sessions. Defaults to $SHELL.
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

---@type string[] Raw command-line arguments passed to zmux, excluding the executable name.
args = {}

---@type ZmuxLaunchState
launch = {}

---@class ZmuxInfoApi
---@type ZmuxInfoApi
inf = {}

---@class ZmuxRuntimeApi
---@field version string Version of the running zmux program.
---@field config_api_version integer Config API version supported by this program.
---@type ZmuxRuntimeApi
zmux = {}
