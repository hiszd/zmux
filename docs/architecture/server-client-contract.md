# Server/client contract

Status: The domain and lifecycle behavior is agreed. The wire protocol is implemented as a versioned, framed local stream and may evolve with its protocol version.

## Ownership

- The background Server owns live Sessions, Workspaces, Tabs, PTYs, terminal state, and Session names.
- A Session contains one or more Workspaces. A Workspace has a working directory and Tabs; new Tabs start there. A Tab's shell changing directories does not update the Workspace directory.
- Active Workspace and active Tab selections are Session state shared by attached Clients. A Session retains its active Workspace while detached or switched away from; each Workspace retains its active Tab. Switching back resumes the previous selection.
- Each Client owns its terminal, viewport, Lua configuration, and local presentation state such as UI Element visibility/focus and modal state.
- Lua `g`/`global` values are read-only snapshots of Server-owned state. `cmd` operations send requests to the Server.

## Connection and lifecycle

- A Client launch without a Session name creates a new Session containing one Workspace at the launch working directory and one initial Tab. The Workspace defaults to the directory basename. Its name is the first unused numeric identifier represented as a string.
- `new NAME` attaches to an existing Session or creates the named Session when it does not exist. `new NAME --strict` fails if the Session already exists. `attach NAME` requires the Session to exist; `--socket NAME` selects an independent Server instance.
- The Client auto-starts the Server in the background. Detach closes only the Client; Sessions remain alive. Stop-server closes all Clients, destroys every Session, and ends the Server. Sessions do not survive restart.
- Closing a Workspace's last Tab closes the Workspace; closing a Session's last Workspace closes the Session. Attached Clients move to the next Session in creation order (wrapping to the oldest remaining Session) or close when no Session remains. The Server exits when its final Session closes.

## IPC

- The shared `interprocess` local-socket stream adapter maps to Unix-domain sockets and Windows named pipes. Unix sockets use a private directory under `$XDG_RUNTIME_DIR`; if it is missing or unusable, zmux warns and uses a private per-user directory under the system temporary directory. Named socket instances are independent.
- Messages are versioned and framed as a four-byte big-endian length followed by a `bincode` payload. Frames are capped at 64 MiB. Each connection carries Client requests, matching replies, and ordered Server updates.
- The initial handshake supplies the optional Session name, launch directory, and Client usable viewport. No target creates a Session; a target attaches to it.
- The Server broadcasts authoritative Session snapshots and PTY output. It sends formatted terminal snapshots on attach and state changes; Clients parse the output for their local viewport and render it.

## Shared PTY sizing

A Tab has one PTY geometry at a time. The Server uses the maximum usable pane rows and columns independently among Clients attached to that Session, recomputing on attach, detach, and resize. It keeps the last size while no Clients are attached. Each Client renders the shared screen in its own viewport; smaller views may clip content and larger views may have unused cells. This is projection, not true scaling or per-client shell reflow. Client-only overlays, including configurable Drawers, never affect PTY dimensions. See [ADR 0002](../adr/0002-shared-pty-sizing.md).

## Workspace and picker interactions

- There is no built-in Workspace Drawer. Workspace navigation is exposed through Lua-bound Workspace commands; custom client-local UI may display and interact with Workspace data.
- Workspace creation/editing uses a directory selector and optional name field. Tab moves to the name field; an empty name derives the basename. Creating a Workspace starts one Tab in the selected directory and activates it. Editing the directory changes only the Workspace default; existing shells keep their current directories.
- The generic Fuzzy Picker accepts a supplied list, fuzzy-filters it, and invokes a Lua completion callback with the selected item or `nil` on Escape.
- Key bindings use neovim-style key notation and Lua callbacks, with sequences after the configured prefix. `cmd.key.bind(sequence, callback, { desc = ... })` binds; `cmd.key.bind(sequence, nil)` unbinds. Sequence timeout defaults to 700 ms. A bottom-anchored full-width popup shows available continuations; unbound sequences and callback failures are shown briefly in yellow.
