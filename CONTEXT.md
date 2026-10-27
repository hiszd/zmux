# zmux Interface Model

This context describes the user-facing model for terminal tabs and configurable interface regions.

## Language

**Server**:
The long-lived zmux process that owns sessions and their terminal processes.
_Avoid_: Client (the attached terminal interface)

**Client**:
A TUI connected to the Server that sends commands and displays current application state.
_Avoid_: Server (the process owning sessions)

**Session**:
A server-owned collection of one or more Workspaces, identified by a unique string name, that retains its active Workspace while Clients detach or switch away. A Session remains alive when its Clients detach and ends when it is killed, its last Workspace closes, or its Server stops.
_Avoid_: Application process (a Server may own multiple Sessions)

**Session Name**:
The string used to identify a Session to users. `new NAME` attaches a Client to an existing Session or creates the named Session when it does not exist; `new NAME --strict` fails if it already exists. Launching without a target creates a Session with the first unused numeric name represented as a string.
_Avoid_: Session Number (numeric names are still strings)

**Workspace**:
A grouping of Tabs within a Session with a working directory used as the starting directory for new Tabs; it retains its last active Tab while another Workspace is selected. A Workspace closes when its last Tab closes. Its directory changes only through Workspace UI; a Tab shell changing its own current directory does not change it. An automatically derived Workspace name tracks the directory basename, while an explicit name is user-controlled.
_Avoid_: Session (the parent collection); Tab (one PTY)

**Tab**:
One PTY-backed terminal within a Workspace; it can be selected by an attached Client. A Tab's shell may change its own current directory independently of the Workspace's directory.
_Avoid_: Session (the top-level collection); Window (ambiguous with terminal-emulator windows)

**Shared PTY Geometry**:
The single row-and-column size used by a Session's Tab PTYs and observed by their shell processes; attached Clients may have different viewports, but cannot provide different PTY sizes to the same Session simultaneously.
_Avoid_: Client Viewport (the rendering area available to one Client)

**Client Viewport**:
The terminal area available to one Client for rendering shared terminal state; it may clip or leave unused space when it differs from the Shared PTY Geometry.
_Avoid_: Shared PTY Geometry (the size observed by terminal processes)

**Detach**:
Disconnecting a Client from its Session while leaving the Session and its terminal processes alive.
_Avoid_: Quit session (the Session continues after detaching)

**Kill Session**:
Ending a Session and all its Workspaces and Tabs, then selecting another Session for attached Clients if one exists.
_Avoid_: Detach (which preserves the Session)

**Stop Server**:
Shutting down the Server and all its Sessions, causing every attached Client to close.
_Avoid_: Detach or Kill Session (which affect a Client or one Session)

**UI Anchor**:
The top or bottom screen edge at which visible Bars reserve one row each. A side-attached Drawer overlays the terminal view rather than reserving space.
_Avoid_: Bar (when referring to the screen edge)

**UI Element**:
A user-configurable, Client-local interface region that displays information and can receive configured keyboard input. Its visibility and focus belong to the Client, while its callbacks may request Application Commands.
_Avoid_: Widget (when referring to a configurable interface region)

**Drawer**:
A UI Element attached to the left or right edge that overlays the terminal view and may receive keyboard focus without changing shared PTY geometry.
_Avoid_: Workspace Drawer (the built-in Workspace-specific UI is removed)

**Layout**:
A nested arrangement of UI content that allocates available space among its children and may contain other Layouts or display primitives.
_Avoid_: Screen layout (when referring to content inside a UI Element)

**Bar**:
A retained, client-local UI Element attached to the top or bottom edge. Each visible Bar reserves one row and owns a styled UI Layout tree; at most two Bars may be created at either edge.
_Avoid_: UI Anchor (when referring to the configurable element)

**UI State**:
Current application values that describe interface conditions, such as the active Workspace, active Tab, leader mode, and terminal status. Session-level selections, including the active Workspace and Tab, are server-authoritative and shared across Clients; client-local presentation state, such as UI Element visibility and focus, belongs to that Client.
_Avoid_: Session state (when referring to visible interface state)

**UI Event**:
An application occurrence, such as a tab being created or selected, that notifies Lua through a callback.
_Avoid_: State (an event is a change/occurrence, not the current values)

**Application Command**:
A request from Lua to perform a supported application operation, such as creating or selecting a tab.
_Avoid_: Event (a command requests a change; an event reports an occurrence)

**Bar Content**:
The nested UI Layout tree attached as the root content of a Bar.
_Avoid_: Status text (when referring to arbitrary bar content)

**Bar Style**:
The base foreground, background, and modifier style applied across a Bar's full row and inherited by its child Text nodes unless overridden.
_Avoid_: Theme (when referring to one Bar's style)

**Fuzzy Picker**:
A generic focus-taking overlay that fuzzy-filters a supplied list of items and invokes its completion callback with the selected item on Enter, or no item on Escape.
_Avoid_: Bar (a Fuzzy Picker temporarily owns keyboard focus)
