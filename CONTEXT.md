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
The string used to identify a Session to users. Specifying an existing name attaches a Client to that Session; `new --attach NAME` creates the named Session when it does not exist. Launching without a target creates a Session with the first unused numeric name represented as a string.
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
A region attached to an edge of the interface that reserves space for interface content. The initial anchors are top and bottom; side anchors, if added later, behave as drawers.
_Avoid_: Bar (when referring to the reserved region)

**Bar**:
The rendered content placed in a UI Anchor. A bar can contain user-configurable content and styling.
_Avoid_: Anchor (when referring to the rendered content)

**Bar Part**:
A distinct content position within a bar: left, center, or right.
_Avoid_: Widget (when referring to configurable bar content)

**UI State**:
Current application values that describe interface conditions, such as the active Workspace, active Tab, leader mode, and terminal status. Session-level selections, including the active Workspace and Tab, are server-authoritative and shared across Clients; client-local presentation state, such as whether a Drawer is expanded, belongs to that Client.
_Avoid_: Session state (when referring to visible interface state)

**UI Event**:
An application occurrence, such as a tab being created or selected, that notifies Lua through a callback.
_Avoid_: State (an event is a change/occurrence, not the current values)

**Application Command**:
A request from Lua to perform a supported application operation, such as creating or selecting a tab.
_Avoid_: Event (a command requests a change; an event reports an occurrence)

**Bar Content**:
Text and styled text segments supplied by the built-in defaults or a user-defined Lua function for a Bar Part.
_Avoid_: Status text (when referring to arbitrary bar content)

**Bar Style**:
The presentation properties chosen for bar content, normally from a default style function and optionally overridden by user configuration using current UI State.
_Avoid_: Theme (when referring to the style resolved for a particular bar part)

**Fuzzy Picker**:
A generic focus-taking overlay that fuzzy-filters a supplied list of items and invokes its completion callback with the selected item on Enter, or no item on Escape.
_Avoid_: Bar (a Fuzzy Picker temporarily owns keyboard focus)

**Workspace Drawer**:
A collapsible left-side overlay listing the current Session's Workspaces. Expanded, it shows full names and takes keyboard focus; arrows move a temporary highlight, Enter switches the Session's active Workspace for all Clients and collapses the Drawer to its one-character-wide rail, and Escape collapses it without switching. The collapsed rail shows each Workspace's initial and highlights the active Workspace. Opening or collapsing the Drawer does not resize PTYs.
_Avoid_: Fuzzy Picker (the Drawer is a persistent navigation element, not a temporary filtered selection dialog)
