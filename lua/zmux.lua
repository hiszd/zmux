---@meta

---@alias ZmuxColor string|integer

---@class ZmuxStyleGroupReference

---@class ZmuxStyle
---@field fg? ZmuxColor|ZmuxStyleGroupReference Foreground color or reference to another group's foreground.
---@field bg? ZmuxColor|ZmuxStyleGroupReference Background color or reference to another group's background.
---@field bold? boolean|ZmuxStyleGroupReference
---@field italic? boolean|ZmuxStyleGroupReference
---@field underline? boolean|ZmuxStyleGroupReference
---@field reverse? boolean|ZmuxStyleGroupReference
---@field remove? ZmuxStyleModifier[] Modifiers to remove from inherited inline styles.

---@alias ZmuxStyleModifier "bold"|"italic"|"underline"|"reverse"

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
---| "PreAttach"
---| "ClientLoaded"
---| "ConfigLoaded"
---| "SessionCreated"
---| "SessionSelected"
---| "SessionClosed"
---| "LastErrorChanged"
---| "WorkspaceCreated"
---| "WorkspaceClosed"
---| "WorkspaceSelected"
---| "TabCreated"
---| "TabClosed"
---| "TabSelected"
---| "LeaderModeChanged"
---| "TerminalTitleChanged"
---| "ProcessExited"
---| "TerminalResized"

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
---@field await? boolean For PreAttach, wait for interactive work requested by this handler before attaching the Client.

---@class ZmuxEventApi
---@field on fun(name: ZmuxEventName|ZmuxEventName[], callback: fun(event: ZmuxEvent), opts?: ZmuxEventOptions) Register a callback for one or more application events.

---@alias ZmuxPickerItem string|{label?: string, text?: string, value?: any}

---@class ZmuxKeyApi
---@field bind fun(keys: string, callback: fun()|nil, opts?: ZmuxKeymapOptions) Bind a neovim-notation key sequence after the configured prefix; pass nil to unbind.

---@alias ZmuxUiSize integer|"auto"|"fill"
---@alias ZmuxElementSize integer|"auto"
---@alias ZmuxDrawerSide "left"|"right"
---@alias ZmuxDrawerWidth ZmuxElementSize
---@alias ZmuxDrawerBorder "none"|"plain"|"rounded"|"double"|"thick"|"light_double_dashed"|"heavy_double_dashed"|"light_triple_dashed"|"heavy_triple_dashed"|"light_quadruple_dashed"|"heavy_quadruple_dashed"|"quadrant_inside"|"quadrant_outside"
---@alias ZmuxToastPosition "top_right"|"top_center"|"bottom_right"|"bottom_center"
---@alias ZmuxBarPosition "top"|"bottom"
---@alias ZmuxHorizontalAlignment "left"|"center"|"right"
---@alias ZmuxUiElementProperty "side"|"width"|"height"|"x"|"y"|"position"|"duration"|"title"|"border"|"border_style"|"style"|"visible"|"content"|"keys"|"on_key"|"on_paste"
---@alias ZmuxStyleSource ZmuxStyle|ZmuxStyleGroupReference
---@alias ZmuxUiElementPropertyValue ZmuxDrawerSide|ZmuxElementSize|integer|string|boolean|ZmuxDrawerBorder|ZmuxStyleSource|ZmuxUiNodeHandle|table<string, fun(element: ZmuxUiElementHandle, key: ZmuxUiKey)>|fun(element: ZmuxUiElementHandle, key: ZmuxUiKey)|fun(element: ZmuxUiElementHandle, text: string)
---@alias ZmuxDrawerProperty ZmuxUiElementProperty
---@alias ZmuxDrawerPropertyValue ZmuxUiElementPropertyValue
---@alias ZmuxTextProperty "text"|"style"
---@alias ZmuxLayoutProperty "children"
---@alias ZmuxSlicesProperty "slices"|"items"
---@alias ZmuxAlignProperty "content"|"horizontal"

---@class ZmuxUiKey
---@field key string Neovim-notation key that triggered the binding.
---@field ctrl boolean Whether Control was pressed.
---@field alt boolean Whether Alt was pressed.
---@field shift boolean Whether Shift was pressed.
---@field is_repeat boolean Whether this is a repeated key event.

---@class ZmuxUiNodeHandle Lua keeps detached nodes alive while this handle is reachable; a parent Layout or Element owns attached nodes.
---@field set fun(property: ZmuxTextProperty|ZmuxLayoutProperty|ZmuxSlicesProperty|ZmuxAlignProperty, value: string|ZmuxStyleSource|ZmuxLayoutChild[]|ZmuxSlicesItems|integer|ZmuxUiNodeHandle|ZmuxHorizontalAlignment) Set a typed property; mutations automatically schedule a redraw.

---@class ZmuxTextOptions
---@field text string Initial text; newlines create additional unwrapped lines.
---@field style? ZmuxStyleSource Initial text style or named style group.

---@class ZmuxTextHandle: ZmuxUiNodeHandle
---@field set fun(property: ZmuxTextProperty, value: string|ZmuxStyleSource) Set text or its style.

---@alias ZmuxLayoutChild ZmuxUiNodeHandle|{node: ZmuxUiNodeHandle, size?: ZmuxUiSize}

---@class ZmuxLayoutHandle: ZmuxUiNodeHandle
---@field set fun(property: ZmuxLayoutProperty, value: ZmuxLayoutChild[]) Replace this Layout's children.

---@class ZmuxSlicesItem
---@field index integer One-based first slice occupied by this item.
---@field slice_width integer Number of consecutive slices occupied.
---@field node ZmuxUiNodeHandle UI node rendered within the assigned slice range.

---@alias ZmuxSlicesItems ZmuxSlicesItem[]

---@class ZmuxSlicesOptions
---@field slices integer Positive number of equal-width horizontal slices.
---@field items ZmuxSlicesItems Slice ranges may be empty but cannot overlap or exceed the slice count.

---@class ZmuxSlicesHandle: ZmuxUiNodeHandle
---@field set fun(property: ZmuxSlicesProperty, value: ZmuxSlicesItems|integer) Replace the slice count or its items.

---@class ZmuxAlignOptions
---@field content ZmuxUiNodeHandle Child tree positioned within the full allocated area.
---@field horizontal? ZmuxHorizontalAlignment Horizontal position; defaults to "left".

---@class ZmuxAlignHandle: ZmuxUiNodeHandle
---@field set fun(property: ZmuxAlignProperty, value: ZmuxUiNodeHandle|ZmuxHorizontalAlignment) Replace the child tree or its alignment.

---@class ZmuxBarOptions
---@field type "bar"
---@field position? ZmuxBarPosition Top or bottom edge. Defaults to "top".
---@field content ZmuxUiNodeHandle One-row UI tree; taller content is an error.
---@field style? ZmuxStyleSource Base style group or style table applied across the full row and inherited by descendants.
---@field visible? boolean Initial visibility. Defaults to false.

---@class ZmuxDrawerOptions
---@field type? "drawer" Element type; defaults to "drawer".
---@field side? ZmuxDrawerSide Edge to attach to. Defaults to "left".
---@field width? ZmuxDrawerWidth Fixed cell width or content-sized "auto". Defaults to "auto".
---@field title? string Optional border title.
---@field border? ZmuxDrawerBorder Ratatui border preset. Defaults to "plain".
---@field border_style? ZmuxStyleSource Border style group or style table.
---@field content ZmuxUiNodeHandle Nested content tree.
---@field keys? table<string, fun(element: ZmuxUiElementHandle, key: ZmuxUiKey)> Direct bindings active while this element is focused.
---@field on_key? fun(element: ZmuxUiElementHandle, key: ZmuxUiKey) Fallback for focused key events without a direct binding.
---@field on_paste? fun(element: ZmuxUiElementHandle, text: string) Handle a whole pasted text payload while focused.
---@field visible? boolean Initial visibility. Defaults to false.

---@class ZmuxModalOptions
---@field type "modal"
---@field width? ZmuxElementSize Width in cells or "auto"; defaults to content width.
---@field height? ZmuxElementSize Height in cells or "auto"; defaults to content height.
---@field title? string Optional border title.
---@field border? ZmuxDrawerBorder Ratatui border preset. Defaults to "plain".
---@field border_style? ZmuxStyleSource Border style group or style table.
---@field content ZmuxUiNodeHandle Nested content tree.
---@field keys? table<string, fun(element: ZmuxUiElementHandle, key: ZmuxUiKey)> Direct bindings; modal input is captured, and unhandled Escape dismisses it by default.
---@field on_key? fun(element: ZmuxUiElementHandle, key: ZmuxUiKey) Fallback for focused key events without a direct binding.
---@field on_paste? fun(element: ZmuxUiElementHandle, text: string) Handle a whole pasted text payload while focused.
---@field visible? boolean Initial visibility. Defaults to false.

---@class ZmuxPopoverOptions
---@field type "popover"
---@field x? integer Horizontal cell offset in the UI area; defaults to 0 and is clamped to fit.
---@field y? integer Vertical cell offset in the UI area; defaults to 0 and is clamped to fit.
---@field width? ZmuxElementSize Width in cells or "auto"; defaults to content width.
---@field height? ZmuxElementSize Height in cells or "auto"; defaults to content height.
---@field title? string Optional border title.
---@field border? ZmuxDrawerBorder Ratatui border preset. Defaults to "plain".
---@field border_style? ZmuxStyleSource Border style group or style table.
---@field content ZmuxUiNodeHandle Nested content tree.
---@field keys? table<string, fun(element: ZmuxUiElementHandle, key: ZmuxUiKey)> Direct bindings; popover input is captured, and unhandled Escape dismisses it by default.
---@field on_key? fun(element: ZmuxUiElementHandle, key: ZmuxUiKey) Fallback for focused key events without a direct binding.
---@field on_paste? fun(element: ZmuxUiElementHandle, text: string) Handle a whole pasted text payload while focused.
---@field visible? boolean Initial visibility. Defaults to false.

---@class ZmuxBottomPanelOptions
---@field type "bottom_panel"
---@field height? ZmuxElementSize Height in cells or "auto"; defaults to content height.
---@field title? string Optional border title.
---@field border? ZmuxDrawerBorder Ratatui border preset. Defaults to "plain".
---@field border_style? ZmuxStyleSource Border style group or style table.
---@field content ZmuxUiNodeHandle Nested content tree.
---@field keys? table<string, fun(element: ZmuxUiElementHandle, key: ZmuxUiKey)> Direct bindings while focused.
---@field on_key? fun(element: ZmuxUiElementHandle, key: ZmuxUiKey) Fallback for focused key events without a direct binding.
---@field on_paste? fun(element: ZmuxUiElementHandle, text: string) Handle a whole pasted text payload while focused.
---@field visible? boolean Initial visibility. Defaults to false.

---@class ZmuxToastOptions
---@field type "toast"
---@field position? ZmuxToastPosition Screen corner or center edge; defaults to "top_right".
---@field duration? integer Visible duration in milliseconds; defaults to 3000.
---@field title? string Optional border title.
---@field border? ZmuxDrawerBorder Ratatui border preset. Defaults to "plain".
---@field border_style? ZmuxStyleSource Border style group or style table.
---@field content ZmuxUiNodeHandle Nested content tree.
---@field visible? boolean Initial visibility. Defaults to false.

---@alias ZmuxUiElementOptions ZmuxBarOptions|ZmuxDrawerOptions|ZmuxModalOptions|ZmuxPopoverOptions|ZmuxBottomPanelOptions|ZmuxToastOptions

---@class ZmuxUiElementHandle This UI Element is disposed after its handle becomes unreachable and Lua collects it; hiding it does not release it.
---@field set fun(property: ZmuxUiElementProperty, value: ZmuxUiElementPropertyValue|nil) Set a typed property or clear an input callback; visual changes automatically schedule a redraw.
---@field show fun() Show this element; Modal and Popover are focused automatically.
---@field hide fun() Hide this element and release its focus if needed.
---@field toggle fun() Toggle visibility; showing Modal and Popover focuses them.
---@field focus fun() Focus this element, showing it if necessary. Bars and Toasts cannot be focused.
---@field blur fun() Return keyboard focus to the terminal; Modal and Popover elements must be hidden instead.

---@alias ZmuxDrawerHandle ZmuxUiElementHandle

---@class ZmuxStyleApi
---@field ref fun(name: string): ZmuxStyleGroupReference Create a reference to a group or to the same property in that group.
---@field create fun(name: string, definition: ZmuxStyle) Create or update an active group; existing UI references update immediately.
---@field define_theme fun(name: string, definitions: table<string, ZmuxStyle>) Register a theme group patch without applying it.
---@field apply_theme fun(name: string) Atomically apply a registered theme; groups it omits remain unchanged.
---@field themes fun(): string[] Return registered theme names in sorted order.

---@class ZmuxUiApi Constructors are available during config loading and mutation-safe callbacks.
---@field pick fun(items: ZmuxPickerItem[], callback: fun(item: any|nil)) Open a generic fuzzy picker from a key or event callback and later call back with the chosen item or nil on Escape.
---@field redraw fun() Request a client redraw.
---@field create fun(options: ZmuxUiElementOptions): ZmuxUiElementHandle Create a Bar, Drawer, Modal, Popover, Bottom Panel, or Toast.
---@field text fun(options: ZmuxTextOptions): ZmuxTextHandle Create a Text node.
---@field row fun(children: ZmuxLayoutChild[]): ZmuxLayoutHandle Create a horizontal Layout.
---@field column fun(children: ZmuxLayoutChild[]): ZmuxLayoutHandle Create a vertical Layout.
---@field slices fun(options: ZmuxSlicesOptions): ZmuxSlicesHandle Partition an area into proportional horizontal slices and assign item ranges.
---@field align fun(options: ZmuxAlignOptions): ZmuxAlignHandle Position a child tree within the full area allocated by its parent.

---@class ZmuxConfigApi
---@field reload fun() Reload the active Client's config file, even when automatic live reload is disabled.

---@class ZmuxClientApi
---@field detach fun() Detach the Client while preserving its Session.

---@class ZmuxServerApi
---@field stop fun() Stop the Server and all Sessions.

---@class ZmuxSessionApi
---@field kill fun() Kill the current Session.
---@field switch fun() Open the Session picker.
---@field select fun(name: string) Switch to an existing Session by name.

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

---@class ZmuxPreAttachSessionApi
---@field rename fun(name: string) Rename the Session selected for this Client.
---@field remove fun() Remove the Session selected for this Client.

---@class ZmuxPreAttachWorkspaceApi
---@field create fun(working_directory: string, name?: string) Create a Workspace in the selected Session before Client attachment.
---@field rename fun(id: string, name: string) Rename a Workspace using a qualified `session:workspace` ID.
---@field setcwd fun(id: string, working_directory: string) Change a Workspace directory using a qualified ID.
---@field remove fun(id: string) Remove a Workspace using a qualified ID.

---@class ZmuxPreAttachTabApi
---@field create fun(workspace_id: string, name?: string) Create a Tab using a qualified `session:workspace` ID.
---@field rename fun(id: string, name: string) Rename a Tab using a qualified `session:workspace:tab` ID.
---@field remove fun(id: string) Remove a Tab using a qualified ID.

---@class ZmuxPreAttachApi
---@field session ZmuxPreAttachSessionApi
---@field workspace ZmuxPreAttachWorkspaceApi
---@field tab ZmuxPreAttachTabApi

---@class ZmuxLaunchState
---@field command string Original CLI subcommand (informational; changing it does not change dispatch).
---@field session? string Session selected for the Client; resolve this before the PreAttach hook.
---@field create_if_missing boolean Whether a missing named Session should be created.
---@field fail_if_exists boolean Whether startup should fail when the named Session already exists.
---@field require_session boolean Whether startup requires an existing selected Session.
---@field working_directory string Working directory used when creating a Session.

---@class ZmuxCommandApi
---@field key ZmuxKeyApi
---@field event ZmuxEventApi
---@field ui ZmuxUiApi
---@field style ZmuxStyleApi
---@field config ZmuxConfigApi
---@field client ZmuxClientApi
---@field server ZmuxServerApi
---@field session ZmuxSessionApi
---@field workspace ZmuxWorkspaceApi
---@field tab ZmuxTabApi
---@field pre_attach ZmuxPreAttachApi Server-owned changes applied during `PreAttach` before Client attachment.

---@class ZmuxWhichOptions
---@field maxcolumns? integer Maximum number of which-key columns. Defaults to 7.
---@field maxheight? integer|string Maximum dialog height in rows or as a percentage string, clamped to 60% of the screen. Defaults to "55%".

---@class ZmuxPtyOptions
---@field border? ZmuxDrawerBorder Border preset around the PTY; defaults to "plain". Use "none" to disable it.
---@field border_style? ZmuxStyleSource Border style group or style table.

---@class ZmuxUIOptions
---@field which? ZmuxWhichOptions
---@field pty? ZmuxPtyOptions

---@class ZmuxTerminalSizeOptions
---@field policy? "latest"|"largest"|"smallest"|"manual" Policy for choosing the Session's one shared PTY geometry; defaults to "latest".
---@field manual? ZmuxTerminalState Fixed rows and columns when policy is "manual".

---@class ZmuxOptions
---@field config_version integer Required config API version; must match `zmux.config_api_version`.
---@field cursor_shape? "block"|"underline"|"bar"
---@field live_reload? boolean Watch the active Client's config file for changes; defaults to false.
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

---@type fun(module: string): any Load a Lua module; prefix with "./" to resolve relative to the importing file.
import = function(module) end

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
