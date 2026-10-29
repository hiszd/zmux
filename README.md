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
cargo run -- workspace list project --socket project
cargo run -- kill project --socket project
cargo run -- server status --socket project
cargo run -- server stop --socket project
```

List the Workspaces in a Session, including the active Workspace and each working directory:

```sh
cargo run -- workspace list project
```

Run Lua against the Server without opening a TUI Client. Choose either a positional file path or inline source with `--string`. Options precede the source; arguments after it are available in Lua as `args[1]`, `args[2]`, and so on. `--path` supplies the launch directory used when a script creates a Session:

```sh
cargo run -- run --socket project --path /path/to/project script.lua first-arg second-arg
cargo run -- run --socket project --path /path/to/project --string 'cmd.session.list(function(s) print(#s) end)' first-arg
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

There is no built-in Workspace Drawer. Workspace selection remains available through Lua-bound commands such as `cmd.workspace.next()` and `cmd.workspace.previous()`; custom navigation UI can be built with the configurable UI Elements. Shared PTYs use the largest attached Client pane dimensions; each Client renders the shared screen in its own viewport.

Workspace creation and editing use a directory selector and optional name field. Tab moves to the name field; an omitted name receives the first unused numeric sibling name. The initial Workspace of a Session defaults to the launch directory basename. Creating a Workspace starts a Tab there and activates the Workspace. The sample config binds Workspace creation/editing and tab navigation; additional commands can be bound in `zmux.lua`.

## Lua configuration

The default user configuration is `zmux/zmux.lua` under the platform's standard config directory: `$XDG_CONFIG_HOME` (or `~/.config`) on Linux and other Unix-like systems, `~/Library/Application Support` on macOS, or `%APPDATA%` on Windows. If that file is absent, zmux uses its built-in defaults; it does not search the current working directory. Set `ZMUX_CONFIG` to select an explicit file (for example, `ZMUX_CONFIG=./zmux.lua`). User config files must declare the supported config API version with `opt.config_version = 3` as the first executable statement (comments may precede it). The program checks this before executing the file; missing or unsupported versions and other load errors stop startup rather than silently falling back. Version 3 uses UpperCamelCase event names such as `TabSelected`, adds focused UI key and paste callbacks, and provides the embedded Lua picker default. When upgrading a version-2 config, change `opt.config_version` to `3`, convert snake_case event names to UpperCamelCase, rename `PreClient` to `PreAttach`, and replace `{ interrupt = true }` with `{ await = true }`. This API version changes when a breaking Lua config API change is introduced, while `zmux.version` reports the running program's release version. Run `zmux config init` to install LuaLS support files (`.luarc.json` and read-only `defs.lua`) and `zmux.lua.example` in the default config directory. All three support files are embedded in the zmux binary; no source checkout is needed. Init preserves existing support files. If `defs.lua` differs from the embedded definition, init asks before replacing it and saves the current version as `defs.old`; use `zmux config init --replace` to confirm non-interactively. Copy `zmux.lua.example` to `zmux.lua` to start customizing. Run `zmux config check` to validate the effective configuration (including `ZMUX_CONFIG`) without starting the Server. LuaLS support for this repository is configured in [`.luarc.json`](.luarc.json), with canonical definitions in [`lua/zmux.lua`](lua/zmux.lua).

Live config reload is opt-in: set `opt.live_reload = true` before starting the Client to watch the effective config file; the default is `false`. Enabling it while watching is off requires a restart, and a successful reload that sets it to `false` stops further watching. Stable file changes reload client key bindings, prefix/sequence settings, UI options, event handlers, and configured UI Elements. You can also request a reload from a runtime callback or key binding with `cmd.config.reload()`, even when automatic watching is disabled. `ConfigLoaded` runs after the initial Client setup and after each successful reload; it receives the current `g` state. `ClientLoaded` remains startup-only, and `PreAttach` is not rerun. Invalid edits leave the previous config active, are logged, and show a temporary warning popup. Changes that only affect startup or Server/session creation do not alter the running session.

Lua's `require("module.name")` search path starts with the directory containing the top-level config file, then falls back to Lua's normal search paths. `import("module.name")` delegates to `require`; prefix a name with `./` to resolve it relative to the importing file instead. For relative imports, dots map to directory separators, `.lua` is added, and modules are cached by resolved file. For example, `import("./keymaps")` loads `keymaps.lua` beside the current file, regardless of the process working directory.

Commands are registered under `cmd`, options under `opt`, and Rust supplies the server-authoritative `g`/`global` snapshot. Register event handlers with `cmd.event.on`, passing either one UpperCamelCase event name or an array of names to share a callback; commands requested by key and event callbacks are applied after the callback returns. The embedded Lua API defaults implement the generic fuzzy picker `cmd.ui.pick(items, callback)`, which users can replace by assigning their own function to `cmd.ui.pick`. It calls back with the selected item on Enter or `nil` on Escape. `LastErrorChanged` is emitted when `g.last_error` is set or cleared, so a status element can update only when that value changes.

Startup customization uses the `PreAttach` lifecycle event. CLI preflight starts the Server and creates or resolves the requested Session using the loaded Lua options. The TUI is initialized next; `PreAttach` handlers then run before the Client attaches, with raw command-line arguments in `args` and the resolved launch settings in `launch`. Mark a handler with `{ await = true }` when attachment must wait for its UI interaction, such as `cmd.ui.pick` or `cmd.session.switch()`. `ConfigLoaded` runs after Client attachment and after each successful reload. `ClientLoaded` runs once after startup commands have settled and before the first normal frame.

```lua
cmd.event.on("PreAttach", function()
    if args[1] == "list" then
        cmd.session.switch()
    end
end, { await = true })

cmd.event.on("ClientLoaded", function()
    -- Client state is available through g; ordinary runtime commands work here.
end)

cmd.event.on("PreAttach", function()
    cmd.pre_attach.workspace.create(launch.working_directory, "review")
end, { await = true })
```

`cmd.pre_attach` provides server-owned Session, Workspace, and Tab mutations that execute before Client attachment. Workspace and Tab operations that identify existing objects use qualified IDs (`session:workspace` and `session:workspace:tab`). Requests run in order after each Lua callback returns; a server error aborts startup rather than silently attaching with a partially applied plan.

Key bindings use neovim-style notation and Lua callbacks: `cmd.key.bind('tw', function() cmd.tab.close() end, { desc = 'Close the focused tab' })`; bind `cmd.config.reload()` to a key to reload on demand. Pass `nil` as the callback to unbind a sequence. Bindings are entered after the required prefix (`opt.prefix_key`, default `<C-Space>`); multi-key sequences are supported and `opt.sequence_timeout_ms` defaults to 700 ms; set it to `0` to disable leader/sequence timeouts. Completing a binding exits leader mode, including when timeouts are disabled; an unbound key also exits leader mode and shows a yellow warning. The Leader key and all subsequent sequence keys always take precedence over focused UI input, including Modal and Popover input. When a sequence has continuations, zmux displays a bottom-anchored, full-width which-key popup. Unknown keys and callback errors appear briefly in yellow at the bottom right.

### Style groups and themes

Each Client starts with the built-in `default` theme and named groups for native UI and the default Bars (`Bar`, `Bar.TabActive`, `Modal.Selected`, `WhichKey.Border`, and others). Rust- and Lua-managed UI resolve these groups through the same per-Client registry. Set a UI style to `cmd.style.ref("GroupName")` to keep a live reference; changing the group updates existing elements on the next redraw.

Built-in groups are `Pty.Border`; `Bar`, `Bar.Tab`, `Bar.TabActive`, `Bar.Leader`, `Bar.LeaderIndicator`, `Bar.Status`, and `Bar.Error`; `Modal.Border`, `Modal.Background`, `Modal.Query`, `Modal.Selected`, `Modal.Item`, and `Modal.Footer`; `WhichKey.Border`, `WhichKey.Background`, `WhichKey.Header`, `WhichKey.Prefix`, `WhichKey.Footer`, `WhichKey.Key`, and `WhichKey.Description`; and `Warning.Key` and `Warning.Reload`.

`cmd.style.create(name, definition)` creates or updates one active group. A property can be a literal color/boolean or `cmd.style.ref("OtherGroup")`, which reads that same property from the other group. Omitted properties resolve to neutral defaults. For example:

```lua
cmd.style.create("MyTab", { fg = "white", bg = "blue" })
cmd.style.create("MyActiveTab", {
    fg = cmd.style.ref("MyTab"),
    bg = "cyan",
    bold = true,
})
```

`cmd.style.define_theme(name, groups)` registers a named set of group definitions but does not switch to it. `cmd.style.apply_theme(name)` applies that set atomically; groups omitted by the theme keep their current definitions. Applying a theme does not remove custom groups. The built-in config binds `<prefix> th` to a fuzzy picker over `cmd.style.themes()`; custom configs can use the same `cmd.ui.pick` pattern.

```lua
cmd.style.define_theme("ocean", {
    ["Bar.TabActive"] = { fg = "black", bg = "#5fd7ff", bold = true },
    ["MyActiveTab"] = { fg = cmd.style.ref("Bar.TabActive") },
})

cmd.key.bind("tm", function()
    cmd.ui.pick(cmd.style.themes(), function(name)
        if name then cmd.style.apply_theme(name) end
    end)
end, { desc = "Choose a style theme" })
```

The PTY border can be customized with `opt.ui.pty.border` using a Ratatui border preset (`"plain"`, `"rounded"`, `"double"`, `"thick"`, and the dashed/quadrant variants), or disabled with `"none"`. Set `opt.ui.pty.border_style` to a style table or `cmd.style.ref("Pty.Border")` to change its color and modifiers. A borderless PTY uses the full panel area for terminal cells.

### Configurable UI Elements

Bars are retained UI Elements created with `cmd.ui.create({ type = "bar", position = "top", content = node, style = style, visible = true })` (use `position = "bottom"` for a bottom Bar). A Bar occupies one row, and at most two can be created at either edge; visible Bars reserve rows in creation order from the edge inward. Hiding a Bar releases its row. Bar content must fit within one row; attempting to create or update a Bar with taller content raises a Lua error. The Bar style fills its entire row, including empty cells, and is inherited by descendant Text nodes. A child inline style overrides only the fields it specifies; use `remove = { "bold" }` to remove an inherited inline modifier. A style-group reference resolves dynamically. A Bar handle can replace its root content with `bar.set("content", node)`, update its style with `bar.set("style", style)`, and control visibility with `bar.set("visible", bool)`, `show()`, `hide()`, or `toggle()`.

`ui.slices({ slices = N, items = { { index = 1, slice_width = 4, node = child } } })` divides its allocated width into N proportional slices. `index` is one-based; each item occupies `slice_width` consecutive slices. Gaps are allowed, but ranges cannot overlap or exceed N. Extra terminal columns are distributed proportionally across slices, and each child is clipped to its assigned range. `ui.align({ content = child, horizontal = "left"|"center"|"right" })` gives its child the full area allocated by its parent and positions the child within it. Both are ordinary nestable Layout nodes; in a `row`, use the existing child `size = "fill"` option when an alignment node should receive the remaining width.

UI Elements and `Text`, `row`, `column`, `slices`, and `align` Layout nodes can be created while loading config or inside runtime event, key, picker-completion, and focused-UI key callbacks. UI handles follow Lua reachability: an Element is disposed after its handle becomes unreachable and Lua collects it; hiding an Element does not release it. Layouts and Elements keep their attached child nodes alive, while a detached node remains reusable as long as Lua retains its handle. Keep a handle in a Lua variable or table for as long as you intend to use it. The built-in Workspace Drawer has been removed; this minimal example creates a custom informational drawer:

```lua
local ui = cmd.ui
local status = ui.text({ text = "Ready" })
local drawer = ui.create({
    type = "drawer",
    side = "right",
    width = "auto",
    title = "Status",
    border = "rounded",
    content = ui.column({ status }),
    keys = {
        r = function(element, key)
            status.set("text", "Handled " .. key.key)
        end,
        q = function(element)
            element.hide()
        end,
    },
})

cmd.key.bind("ui", function()
    drawer.show()
    drawer.focus()
end, { desc = "Show status drawer" })
```

Runtime-created Elements need a retained Lua handle for as long as they should remain available. Assigning a replacement or `nil` makes the old handle eligible for Lua garbage collection; hiding an Element alone keeps it alive.

```lua
local active_dialog
cmd.event.on("WorkspaceSelected", function()
    if active_dialog then
        active_dialog.hide()
        active_dialog = nil
    end
    active_dialog = cmd.ui.create({
        type = "modal",
        content = cmd.ui.text({ text = g.active_workspace.name }),
        visible = true,
    })
end)
```

`Text`, `row`, and `column` constructors return handles. Layout children may be auto-sized, fixed in terminal cells, or set to `"fill"` to share remaining space equally. `ui.create` also supports `modal`, `popover`, `bottom_panel`, and `toast` alongside `drawer`. Modals center over the terminal panel, dim its background, take focus when shown, and capture keys until hidden; unhandled Escape dismisses them by default. A Popover is positioned by `x` and `y` cell offsets in that panel, clamps to fit, and also captures keys. A Bottom Panel overlays the bottom of the panel rather than reserving rows. Toasts are non-focusable and dismiss automatically after `duration` milliseconds (default 3000); positions are `top_right`, `top_center`, `bottom_right`, or `bottom_center`. All accept nested retained content, title, border, and border style.

For example, all four patterns use the same constructor and handle lifecycle:

```lua
local ui = cmd.ui
local dialog = ui.create({
    type = "modal",
    title = "Confirm",
    content = ui.column({ ui.text({ text = "Close this tab?" }) }),
    keys = { y = function(element) element.hide(); cmd.tab.close() end },
})
local menu = ui.create({
    type = "popover", x = 8, y = 2,
    content = ui.column({ ui.text({ text = "Actions" }) }),
})
local output = ui.create({
    type = "bottom_panel", height = 5,
    content = ui.column({ ui.text({ text = "Command output" }) }),
})
local notice = ui.create({
    type = "toast", position = "bottom_right", duration = 2500,
    content = ui.text({ text = "Saved" }),
})

cmd.key.bind("q", function()
    dialog.show() -- Modal and Popover focus automatically.
end)
cmd.key.bind("m", function() menu.show() end)
cmd.key.bind("o", function() output.show() end)
cmd.key.bind("s", function() notice.show() end)
```

Focusable UI Elements may provide direct `keys` bindings, an `on_key(element, key)` fallback for otherwise-unbound keys, and an `on_paste(element, text)` callback for a complete paste payload. Direct bindings run before `on_key`. A capturing Modal or Popover consumes unhandled keys and paste; non-capturing Elements can let unhandled paste reach the PTY. Paste while Leader mode is active is swallowed, and key input remains with the global sequence dispatcher. When the PTY is focused, paste is sent as text, wrapped in bracketed-paste markers only when the terminal application has enabled that mode. A no-op function consumes a local binding; omitting a key removes it. Setters such as `status.set("text", "Updated")` and `drawer.set("border", "double")` invalidate the retained UI and schedule a coalesced redraw.

List global bindings from the loaded configuration with `zmux keybinds`; use `--json` for pretty-printed JSON on stdout. Each entry includes its sequence and optional description, and the configured prefix is reported separately. Focus-scoped UI Element bindings are not included in this listing. This command loads the Lua config but does not connect to or start the Server.

## Tracing

Enable latency tracing without corrupting the TUI:

```sh
ZMUX_TRACE=1 cargo run
```

Trace output is written to `zmux-trace.log`; set `ZMUX_TRACE_LOG` to choose another path. Remote access is deferred; see [`docs/remote-access.md`](docs/remote-access.md).
