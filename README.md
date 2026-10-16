# zmux

A local terminal multiplexer with a background Server, multiple TUI Clients, PTY-backed Tabs, and a Lua-configurable interface.

## Run

```sh
cargo run
```

The command is required. Create a new Session with an automatically generated numeric name, one Workspace, and one Tab. `--path` sets the initial Workspace directory; without it, zmux uses the current directory:

```sh
cargo run -- new --path /path/to/project
```

Create or attach to a named Session:

```sh
cargo run -- new project
```

`new` uses ensure semantics: it attaches to the requested Session when it exists, or creates a new Session when it does not. Add `--strict` to fail when the named Session already exists:

```sh
cargo run -- new project --strict
```

Attach to an existing Session by name with the explicit attach command:

```sh
cargo run -- attach project
```

A named socket instance runs an independent Server and Session set:

```sh
cargo run -- new --socket project
cargo run -- attach project --socket project
cargo run -- list --socket project
cargo run -- kill project --socket project
cargo run -- server status --socket project
cargo run -- server stop --socket project
```

Run a Lua script against the Server without opening a TUI Client. Options precede the script; arguments after the script are available in Lua as `args[1]`, `args[2]`, and so on. `--path` supplies the launch directory used when a script creates a Session:

```sh
cargo run -- run --socket project --path /path/to/project script.lua first-arg second-arg
```

The script runs in a clean Lua environment and does not load `zmux.lua`. Its Server-owned operations use `cmd.session`, `cmd.workspace`, `cmd.tab`, and `cmd.server`; list calls accept callbacks. Selectors use qualified IDs (`session`, `session:workspace`, `session:workspace:tab`). Names cannot contain `:`. Workspace and Tab names omitted at creation use the first unused numeric sibling name; the initial Workspace defaults to the launch directory basename. For example:

```lua
cmd.session.create("project", { workspace_name = "main" })
cmd.workspace.list("project", function(workspaces)
    print(workspaces[1].id, workspaces[1].working_directory)
end)
cmd.server.stop()
```

The Server starts automatically in the background and remains alive while Sessions exist, including when all Clients detach. Sessions are ephemeral: stopping the Server destroys them. Unix-like systems use a socket under `$XDG_RUNTIME_DIR`, falling back to a private per-user runtime directory with a warning; Windows uses a user-scoped named pipe.

## Session model

- **Session**: named collection of Workspaces. Sessions created without a name receive the first unused numeric name represented as a string. Active Workspace and Tab state is shared across attached Clients and resumes after detach or switching away.
- **Workspace**: collection of Tabs with a working directory. Its display name defaults to the directory basename, unless explicitly named. New Tabs start there; changing a shell's current directory does not change the Workspace directory.
- **Tab**: exactly one PTY-backed terminal.

The left Workspace Drawer is always visible as a one-character rail. A configured binding expands it to show full names and take focus; arrows move the temporary highlight, Enter switches the shared active Workspace and collapses the Drawer, and Escape collapses without switching. The Drawer overlays the terminal view and does not resize PTYs. Shared PTYs use the largest attached Client pane dimensions; each Client renders the shared screen in its own viewport.

Workspace creation and editing use a directory selector and optional name field. Tab moves to the name field; an omitted name receives the first unused numeric sibling name. The initial Workspace of a Session defaults to the launch directory basename. Creating a Workspace starts a Tab there and activates the Workspace. The sample config binds Drawer toggle, Workspace creation/editing, and tab navigation; additional commands can be bound in `zmux.lua`.

## Lua configuration

The default configuration is `./zmux.lua`; set `ZMUX_CONFIG` to load another file. LuaLS support is configured in [`.luarc.json`](.luarc.json), with definitions in [`lua/zmux.lua`](lua/zmux.lua).

Commands are registered under `cmd`, options under `opt`, and Rust supplies the server-authoritative `g`/`global` snapshot. Register event handlers with `cmd.event.on`; commands requested by key and event callbacks are applied after the callback returns. The generic fuzzy picker is `cmd.ui.pick(items, callback)`: it calls back with the selected item on Enter or `nil` on Escape.

Startup customization uses two lifecycle events. `PreClient` runs before connecting and showing the interactive Client; the raw command-line arguments are available in `args`, and mutable launch settings are in `launch`. For example, `launch.session` selects the Session to attach to. Mark a `PreClient` handler with `{ interrupt = true }` when startup must wait for its built-in interactive work, such as `cmd.ui.pick` or `cmd.session.switch()`. Other runtime commands from `PreClient` are applied in order after the selected Session is connected and before the first normal frame. `ClientLoaded` runs after those startup commands have settled and before that first frame.

```lua
cmd.event.on("PreClient", function()
    if args[1] == "list" then
        cmd.session.switch()
    end
end, { interrupt = true })

cmd.event.on("ClientLoaded", function()
    -- Client state is available through g; ordinary runtime commands work here.
end)
```

Key bindings use neovim-style notation and Lua callbacks: `cmd.key.bind('tw', function() cmd.tab.close() end, { desc = 'Close the focused tab' })`. Pass `nil` as the callback to unbind a sequence. Bindings are entered after the required prefix (`opt.prefix_key`, default `<C-Space>`); multi-key sequences are supported and `opt.sequence_timeout_ms` defaults to 700 ms; set it to `0` to disable leader/sequence timeouts. Completing a binding exits leader mode, including when timeouts are disabled; an unbound key also exits leader mode and shows a yellow warning. `<Esc>` cannot be bound because it is reserved for cancellation. When a sequence has continuations, zmux displays a bottom-anchored, full-width which-key popup. Unknown keys and callback errors appear briefly in yellow at the bottom right.

## Tracing

Enable latency tracing without corrupting the TUI:

```sh
ZMUX_TRACE=1 cargo run
```

Trace output is written to `zmux-trace.log`; set `ZMUX_TRACE_LOG` to choose another path. Remote access is deferred; see [`docs/remote-access.md`](docs/remote-access.md).
