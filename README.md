# zmux

A local terminal multiplexer with a background Server, multiple TUI Clients, PTY-backed Tabs, and a Lua-configurable interface.

## Run

```sh
cargo run
```

The command is required. Create a new numbered Session with one Workspace and one Tab:

```sh
cargo run -- new
```

Attach to an existing Session by number:

```sh
cargo run -- attach 2
```

`new --attach` uses ensure semantics: it attaches to the requested Session when it exists, or creates a new Session when it does not:

```sh
cargo run -- new --attach 2
```

A named socket instance runs an independent Server and Session set:

```sh
cargo run -- new --socket project
cargo run -- attach 1 --socket project
cargo run -- list --socket project
cargo run -- kill 1 --socket project
cargo run -- server status --socket project
cargo run -- server stop --socket project
```

The Server starts automatically in the background and remains alive while Sessions exist, including when all Clients detach. Sessions are ephemeral: stopping the Server destroys them. Unix-like systems use a socket under `$XDG_RUNTIME_DIR`, falling back to a private per-user runtime directory with a warning; Windows uses a user-scoped named pipe.

## Session model

- **Session**: sequentially numbered collection of Workspaces. Active Workspace and Tab state is shared across attached Clients and resumes after detach or switching away.
- **Workspace**: collection of Tabs with a working directory. Its display name defaults to the directory basename, unless explicitly named. New Tabs start there; changing a shell's current directory does not change the Workspace directory.
- **Tab**: exactly one PTY-backed terminal.

The left Workspace Drawer is always visible as a one-character rail. A configured binding expands it to show full names and take focus; arrows move the temporary highlight, Enter switches the shared active Workspace and collapses the Drawer, and Escape collapses without switching. The Drawer overlays the terminal view and does not resize PTYs. Shared PTYs use the largest attached Client pane dimensions; each Client renders the shared screen in its own viewport.

Workspace creation and editing use a directory selector and optional name field. Tab moves to the name field; leaving it blank derives the name from the selected directory. Creating a Workspace starts a Tab there and activates the Workspace. The sample config binds Drawer toggle, Workspace creation/editing, and tab navigation; additional commands can be bound in `zmux.lua`.

## Lua configuration

The default configuration is `./zmux.lua`; set `ZMUX_CONFIG` to load another file. LuaLS support is configured in [`.luarc.json`](.luarc.json), with definitions in [`lua/zmux.lua`](lua/zmux.lua).

Commands are registered under `cmd`, options under `opt`, and Rust supplies the server-authoritative `g`/`global` snapshot. Register event handlers with `cmd.on`; commands requested by key and event callbacks are applied after the callback returns. The generic fuzzy picker is `cmd.pick(items, callback)`: it calls back with the selected item on Enter or `nil` on Escape.

Key bindings use neovim-style notation and Lua callbacks: `cmd.bind('tw', function() cmd.close_tab() end, { desc = 'Close the focused tab' })`. Pass `nil` as the callback to unbind a sequence. Bindings are entered after the required prefix (`opt.prefix_key`, default `<C-Space>`); multi-key sequences are supported and `opt.sequence_timeout_ms` defaults to 700 ms; set it to `0` to disable leader/sequence timeouts. Completing a binding exits leader mode, including when timeouts are disabled; an unbound key also exits leader mode and shows a yellow warning. `<Esc>` cannot be bound because it is reserved for cancellation. When a sequence has continuations, zmux displays a bottom-anchored, full-width which-key popup. Unknown keys and callback errors appear briefly in yellow at the bottom right.

## Tracing

Enable latency tracing without corrupting the TUI:

```sh
ZMUX_TRACE=1 cargo run
```

Trace output is written to `zmux-trace.log`; set `ZMUX_TRACE_LOG` to choose another path. Remote access is deferred; see [`docs/remote-access.md`](docs/remote-access.md).
