# Implementation plan

Status: Implementation in progress. The behavior contract is agreed; this plan tracks the staged move from the in-process prototype to the Server/Client architecture.

## Goal and scope

- A background Server owns ephemeral Sessions and their terminal processes.
- A Session has an automatically assigned sequential number and contains one or more Workspaces. A Workspace has a working directory and contains Tabs. A Tab is one PTY.
- A Client launch without a Session Number creates a new Session. A launch specifying an existing number attaches to it.
- Session state, including active Workspace and active Tab, is shared across all attached Clients. Client-local presentation state, including Workspace Drawer visibility and its pending highlight, is not shared.
- Detach preserves Sessions; stopping the Server closes clients and destroys all Sessions. No database, durable recovery, or remote access is in scope.
- New Tabs start in their Workspace's working directory. Shell `cd` does not change the Workspace directory. Workspace directory edits happen through the UI; derived Workspace names follow the directory basename, while explicit names remain user-controlled.
- Unix-like systems use a per-user socket under `$XDG_RUNTIME_DIR`, with a private per-user fallback directory and a warning if the runtime directory is missing or unusable. Windows uses a user-scoped named pipe behind the same transport interface.

## Phases

### 0. Close the remaining contract gaps — complete

The following behavior decisions are agreed:

- **Agreed — Session numbering:** allocate monotonically from 1 for one Server lifetime, do not reuse numbers after a Session is killed, and start again at 1 after Server restart.
- **Agreed — CLI attachment:** `zmux --session N` attaches to an existing Session; an unknown number is an error. A launch without the flag creates a new Session.
- **Agreed — initial contents:** create one Workspace rooted at the Client's launch working directory, with one initial Tab started there; the Workspace defaults to the directory basename.
- **Agreed — cascading closure:** closing a Workspace's last Tab closes that Workspace; closing a Session's last Workspace closes that Session; closing the last Session shuts down the Server.
- **Agreed — new Tab selection:** a newly created Tab becomes active immediately in its Workspace, with the selection shared across attached Clients.
- **Agreed — Fuzzy Picker result delivery:** the generic Picker accepts a supplied list and later invokes a completion callback with the selected item or `nil` on Escape.
- **Agreed — key bindings:** bindings use neovim-style key notation, Lua callbacks, a required prefix, multi-key sequences, optional descriptions, unbinding via a nil callback, and a 700 ms default sequence timeout.

### 1. Extract and test the domain model

Introduce a library-facing domain module so tests and the future Server use the same model. Keep PTY/process operations outside the pure state transitions.

- Model Session Number, Session, Workspace, Tab identity, active Workspace, and active Tab.
- Store an optional explicit Workspace name separately from the working directory; derive the visible name from the directory basename when no override exists.
- Define operations to create/edit Workspaces and select Workspaces/Tabs. Selection is authoritative Session state, so every attached Client observes the same result. A Session retains its active Workspace; each Workspace retains its active Tab, including while detached or switched away from. Closing an active Tab selects the Tab that moves into its position, or the previous Tab if the removed Tab was last. Closing an active Workspace follows the same rule among the remaining Workspaces.
- Add behavior tests for derived-name updates, explicit-name retention, Tab startup directory selection, selection validity, and Session/Workspace lifecycle invariants.

### 2. Define the protocol and transport seam

- Define versioned request, reply, and server-event messages. Include an initial state snapshot, ordered state changes, terminal input/output or screen updates, resize notifications, detach, and shutdown.
- Keep framing and serialization independent of the transport implementation.
- Define one transport interface with Unix-domain socket and Windows named-pipe adapters. Unix socket setup validates ownership and permissions; the fallback directory is private and warns when used. Named socket instances identify independent Servers.
- Test message framing, protocol-version errors, disconnects, startup races, and secure socket-path behavior through the public transport interface.

### 3. Move ownership into the background Server

- Extract a Server state manager that owns Sessions, Workspaces, Tabs, PTYs, terminal parsers, and client registrations.
- Start the Server automatically when a Client cannot connect; serialize startup so concurrent Clients do not create competing default Servers.
- Create a new sequentially numbered Session when a Client launches without a target; attach to the specified existing Session when a number is supplied.
- Route tab/workspace commands to the Server and broadcast authoritative state changes to all Clients attached to the affected Session.
- Implement detach, kill-session, and stop-server semantics. Killing a Session moves its Clients to the next Session in creation order, wrapping to the oldest remaining Session when needed, or closes them if none remains; destroying the last Session shuts down the Server.
- Use the agreed shared terminal sizing rule: maximum usable rows and columns independently across attached Clients in the Session; recompute on attach/detach/resize and retain the last size when no Clients remain. Apply that geometry to all Session Tabs and notify their processes of resizes.

### 4. Convert the current TUI into an IPC Client

- The Client owns terminal raw mode, keyboard input, rendering, Lua configuration, and per-client viewport dimensions; it no longer spawns or owns PTYs.
- Send input/commands to the Server and consume initial snapshots and ordered updates.
- Render the shared terminal state in each Client's own viewport. Smaller viewports may clip the shared PTY screen and larger ones may have unused space; this is projection, not per-client shell reflow or glyph scaling. Client-only overlays such as the Workspace Drawer never affect PTY geometry; reserving layout regions do affect that Client's usable viewport.
- Ensure terminal-mode cleanup on detach, connection failure, Server shutdown, and errors.
- Add integration tests for attach, detach/reattach, multiple Clients, state broadcast, per-client projection, and server shutdown.

### 5. Add Workspace and picker interactions

- Implement the Workspace Drawer as a left-side overlay that does not resize PTYs. Expanded: full names, focus captured, arrows move a temporary highlight, Enter commits the shared Workspace selection and collapses to the rail, Escape collapses without switching. Collapsed: a one-character-wide rail shows the first character of each Workspace name and highlights the active Workspace.
- Implement Workspace create/edit dialogs with a working-directory selector and optional name field. Tab moves to the name field; an empty name uses the directory basename. Creating a Workspace spawns one initial Tab in the chosen directory and immediately activates the new Workspace. Create and edit are separately bindable actions.
- Implement the generic Fuzzy Picker as a reusable modal over a supplied list. Enter invokes the completion callback with the selected item; Escape invokes it with `nil`. Keep it generic rather than adding picker-specific binding behavior.
- Test the UI state machine independently from terminal rendering, then add manual TUI tests for focus, overlay, selection, cancellation, and narrow terminals.

### 6. Preserve and extend the Lua configuration contract

- Keep `cmd`, `opt`, and server-authoritative `g`/`global` state. Add Session/Workspace/Tab snapshot types and operations without exposing mutable server state to Lua.
- Add actions for Workspace Drawer toggle, Workspace creation/edit, detach, kill-session, and stop-server. Leave key values unbound in the sample config for the user to choose.
- Add typed Lua helpers for the generic Fuzzy Picker and document its asynchronous result handling.
- Test sample config loading, Lua type declarations, callback ordering, and rejected commands.

### 7. End-to-end verification

- Verify on Unix and Windows: default-server startup, named independent servers, explicit Session attachment, no-target Session creation, concurrent clients, detach/reattach, Workspace and Tab switching, kill-session fallback, stop-server destruction, and Server exit when no Sessions remain.
- Verify largest-client PTY sizing with clients of different sizes, including attach/detach/resize and the invariant that opening the Drawer never resizes a PTY.
- Run `cargo fmt`, `cargo check`, `cargo test`, and `cargo clippy --no-deps` for each Rust implementation milestone; add platform CI and manual terminal QA before calling the multiplexer complete.

## Implementation order

Implement one vertical slice at a time: domain transition tests first, then a functioning Unix transport/Server/Client path, then the Windows transport adapter, then Workspace UI and Lua integration. Keep protocol types and domain state independent of platform adapters so the Windows and Unix paths share the same behavior tests.
