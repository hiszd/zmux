# Server/client contract

Status: The domain and lifecycle behavior is agreed. The wire protocol is implemented as a versioned, framed local stream and may evolve with its protocol version.

## Ownership

- The background Server owns live Sessions, Workspaces, Tabs, PTYs, terminal state, and Session numbering.
- A Session contains one or more Workspaces. A Workspace has a working directory and Tabs; new Tabs start there. A Tab's shell changing directories does not update the Workspace directory.
- Active Workspace and active Tab selections are Session state shared by attached Clients. A Session retains its active Workspace while detached or switched away from; each Workspace retains its active Tab. Switching back resumes the previous selection.
- Each Client owns its terminal, viewport, Lua configuration, and local presentation state such as Drawer visibility, pending highlight, and modal state.
- Lua `g`/`global` values are read-only snapshots of Server-owned state. `cmd` operations send requests to the Server.

## Connection and lifecycle

- A Client launch without a Session Number creates a new Session containing one Workspace at the launch working directory and one initial Tab. The Workspace defaults to the directory basename.
- Session Numbers are allocated monotonically from 1 for a Server lifetime, are not reused after a Session closes, and reset to 1 after Server restart.
- `zmux --session N` attaches to an existing Session; an unknown number is an error. `--socket NAME` selects an independent Server instance.
- The Client auto-starts the Server in the background. Detach closes only the Client; Sessions remain alive. Stop-server closes all Clients, destroys every Session, and ends the Server. Sessions do not survive restart.
- Closing a Workspace's last Tab closes the Workspace; closing a Session's last Workspace closes the Session. Attached Clients move to the next Session in creation order (wrapping to the oldest remaining Session) or close when no Session remains. The Server exits when its final Session closes.

## IPC

- The shared `interprocess` local-socket stream adapter maps to Unix-domain sockets and Windows named pipes. Unix sockets use a private directory under `$XDG_RUNTIME_DIR`; if it is missing or unusable, zmux warns and uses a private per-user directory under the system temporary directory. Named socket instances are independent.
- Messages are versioned and framed as a four-byte big-endian length followed by a `bincode` payload. Frames are capped at 64 MiB. Each connection carries Client requests, matching replies, and ordered Server updates.
- The initial handshake supplies the optional Session Number, launch directory, and Client usable viewport. No target creates a Session; a target attaches to it.
- The Server broadcasts authoritative Session snapshots and PTY output. It sends formatted terminal snapshots on attach and state changes; Clients parse the output for their local viewport and render it.

## Shared PTY sizing

A Tab has one PTY geometry at a time. The Server uses the maximum usable pane rows and columns independently among Clients attached to that Session, recomputing on attach, detach, and resize. It keeps the last size while no Clients are attached. Each Client renders the shared screen in its own viewport; smaller views may clip content and larger views may have unused cells. This is projection, not true scaling or per-client shell reflow. Client-only overlays, including the Workspace Drawer, never affect PTY dimensions. See [ADR 0002](../adr/0002-shared-pty-sizing.md).

## Workspace and picker interactions

- The left Workspace Drawer is always visible as a one-character rail and expands by a configurable binding. Expanded, it shows full names and takes focus; arrows move a temporary highlight, Enter switches the shared active Workspace and collapses the Drawer, and Escape collapses without switching.
- The Drawer highlights the active Workspace in the rail and overlays the terminal without resizing PTYs.
- Workspace creation/editing uses a directory selector and optional name field. Tab moves to the name field; an empty name derives the basename. Creating a Workspace starts one Tab in the selected directory and activates it. Editing the directory changes only the Workspace default; existing shells keep their current directories.
- The generic Fuzzy Picker accepts a supplied list, fuzzy-filters it, and invokes a Lua completion callback with the selected item or `nil` on Escape.
- Key bindings use neovim-style key notation and Lua callbacks, with sequences after the configured prefix. `cmd.bind(sequence, callback, { desc = ... })` binds; `cmd.bind(sequence, nil)` unbinds. Sequence timeout defaults to 700 ms. A bottom-anchored full-width popup shows available continuations; unbound sequences and callback failures are shown briefly in yellow.
