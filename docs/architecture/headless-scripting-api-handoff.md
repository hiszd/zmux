# Headless Lua Scripting API — Implementation Handoff

## Destination

Implement a headless CLI scripting mode that executes Lua against the zmux Server without starting or affecting a TUI Client. The implementation should be executable as one focused coding session from this document.

The headless API reuses the existing Lua command vocabulary and asynchronous command model, but exposes only Server-owned operations. It must support scripting Session, Workspace, and Tab lifecycle and selection operations.

## Invocation

```sh
zmux run [--socket NAME] [--path DIRECTORY] SCRIPT.lua [SCRIPT_ARGS...]
```

- `run` executes `SCRIPT.lua` without starting a TUI Client.
- `--socket NAME` selects the existing named Server instance, using the same socket mechanism as other CLI commands.
- `--path DIRECTORY` supplies the launch working directory used for defaults.
- The process working directory is the default launch directory when `--path` is omitted.
- Arguments after the script path are exposed through the existing global Lua table `args`, preserving their order.
- `zmux.lua` is **not** loaded or executed. The script runs in a clean Lua environment containing only the headless scripting API and its headless state.

## Scope and non-goals

### Included

- Server-owned Session, Workspace, and Tab operations.
- Server stop.
- Asynchronous list queries with callbacks.
- Shared active Workspace and Tab selection.
- Qualified IDs and explicit targeting.
- Server replies, errors, and state needed to drive callbacks.

### Excluded

- Starting, rendering, or otherwise controlling a TUI Client.
- Client attachment or detachment.
- UI anchors, bars, pickers, drawers, key bindings, redraws, and other presentation state.
- A server-global active Session: Sessions are not globally focused on the Server.
- Synchronous Lua command results.
- Executing the user's `zmux.lua` configuration.

`cmd.session.focus()` may remain available for the interactive TUI's existing Client switching behavior, but it must return an error in headless mode. It is not a headless Server operation.

## Domain and identity model

Use the existing domain terms: **Server**, **Session**, **Workspace**, **Tab**, and **Client**. The Server owns Sessions, Workspaces, Tabs, PTYs, and active Workspace/Tab state. A Client is the attached TUI.

### Names and qualified IDs

Every Session, Workspace, and Tab has a user-facing name and a qualified ID:

```text
Session:  bob
Workspace: bob:larry
Tab:       bob:larry:oscar
```

Rules:

- `:` is reserved as the hierarchy separator and is forbidden in all user-controlled names.
- Empty names are invalid.
- Session names are unique globally.
- Workspace names are unique within a Session.
- Tab names are unique within a Workspace.
- Explicit command selectors use the fully qualified ID.
- Listing results expose both the local `name` and fully qualified `id`.
- Qualified IDs are derived from current names. Renaming a parent changes its ID and all descendant IDs.
- Scripts must not expect an ID to survive a rename; they should use the new ID.
- Automatic names are assigned once at creation and remain unchanged when sibling list positions change.
- Automatic numeric names use the first unused numeric sibling name (`"1"`, `"2"`, etc.).

The current implementation uses numeric internal IDs and does not give Tabs explicit user-controlled names. Replace or adapt that model so stable user-facing names and qualified IDs are available to the command layer and protocol. Do not use live list indexes as names.

A Tab's scriptable `name` is separate from its live terminal/process `title`.

## Lua API

Use the existing `cmd` namespace, with canonical verbs:

- `create`
- `list`
- `rename`
- `focus`
- `remove`

This API is not yet stable, so existing names such as `close`, `select`, `switch`, and `edit` do not need compatibility aliases. Use the canonical names in new code and documentation.

### Target resolution

For every operation with an optional target:

- An explicit qualified ID always wins.
- In TUI mode, an omitted target resolves through the active values in the existing read-only `g` snapshot.
- In headless mode, there is no active Session, Workspace, or Tab. Omitting a required target is an error.
- Do not add a second mutable “current target” global. Use `g.session`, `g.active_workspace`, and the corresponding existing snapshot values for TUI implicit context.

### Session commands

```lua
cmd.session.create(session_id?, options?)
cmd.session.list(callback)
cmd.session.rename(session_id?, new_name)
cmd.session.remove(session_id?)
cmd.session.focus(session_id?) -- TUI Client operation; error in headless mode
```

Session creation:

- The optional first positional argument is the Session name/ID.
- `options` is optional.
- The Session name receives a stable numeric default if omitted.
- A new Session is created atomically with an initial Workspace and initial Tab.
- The initial Workspace directory defaults to the launch directory (TUI start directory, or `--path` for CLI invocations).
- The initial Workspace name defaults to the final directory component.
- `options.workspace_name` optionally overrides the derived Workspace name.
- There is no required `working_directory` field for Session creation because the launch directory supplies the default.

Example:

```lua
cmd.session.create("bob", {
  workspace_name = "project",
})
```

### Workspace commands

```lua
cmd.workspace.create(session_id?, options)
cmd.workspace.list(session_id?, callback)
cmd.workspace.rename(workspace_id?, new_name)
cmd.workspace.setcwd(workspace_id?, working_directory)
cmd.workspace.focus(workspace_id?)
cmd.workspace.remove(workspace_id?)
```

Workspace creation:

- The optional first positional argument selects the parent Session by qualified ID (`"bob"`).
- `options` is required in both TUI and headless modes.
- `options.working_directory` is required in both modes.
- `options.name` is optional; omission assigns the first unused numeric sibling name.
- Creating a Workspace also creates its initial Tab and makes the new Workspace active in the Session.

Example:

```lua
cmd.workspace.create("bob", {
  name = "larry",
  working_directory = "/work/project",
})
```

`setcwd` changes the Workspace's default directory for future Tabs. Existing shell processes keep their current directories. If the Workspace name was automatically derived, it tracks the new directory basename; an explicit name remains unchanged.

### Tab commands

```lua
cmd.tab.create(workspace_id?, options?)
cmd.tab.list(workspace_id?, callback)
cmd.tab.rename(tab_id?, new_name)
cmd.tab.focus(tab_id?)
cmd.tab.remove(tab_id?)
```

Tab creation:

- The optional first positional argument selects the parent Workspace by qualified ID (`"bob:larry"`).
- `options` is optional.
- `options.name` is optional; omission assigns the first unused numeric sibling name.
- In TUI mode, omitting the Workspace target creates the Tab in `g.active_workspace`.
- In headless mode, omitting the Workspace target is an error.
- A new Tab starts in the parent Workspace's current working directory.

Example:

```lua
cmd.tab.create("bob:larry", { name = "oscar" })
```

### Server commands

```lua
cmd.server.stop()
```

`cmd.server.stop()` is Server-owned and available headlessly. The Server should flush/send acceptance before shutting down. Headless mode treats an accepted stop request as success even though the connection then closes. A transport failure before acceptance is an error.

## Lists and asynchronous callbacks

All Lua operations remain asynchronous. No command returns a synchronous result table.

List calls use callbacks and pass an array of plain Lua tables:

```lua
cmd.session.list(function(sessions)
  for _, session in ipairs(sessions) do
    print(session.id, session.name)
  end
end)

cmd.workspace.list("bob", function(workspaces)
  for _, workspace in ipairs(workspaces) do
    print(workspace.id, workspace.name, workspace.working_directory)
  end
end)

cmd.tab.list("bob:larry", function(tabs)
  for _, tab in ipairs(tabs) do
    print(tab.id, tab.name, tab.title)
  end
end)
```

Expected common fields:

- Session: `id`, `name`, and useful existing summary fields such as Workspace/Tab counts.
- Workspace: `id`, `name`, `working_directory`, and relevant active/name metadata.
- Tab: `id`, `name`, `title`, and relevant process/exit metadata.

List callbacks may issue more commands. Callback execution and command dispatch must use the same asynchronous queue model as existing Lua callbacks.

## Headless runtime behavior

Implement a headless Lua runtime that shares Lua API registration and command concepts with the existing config runtime, but does not create a TUI or load `zmux.lua`.

Use a dedicated headless Server command connection rather than attaching as a TUI Client or using only the existing global admin connection. The connection:

- attaches to no Session;
- receives no terminal output or viewport state;
- sends explicit-target Server requests;
- receives command acknowledgements, list responses, relevant state updates, and errors;
- reuses the existing transport and framing infrastructure;
- supports the existing `--socket` selection.

Execution order:

1. Parse `run` options and script arguments.
2. Create the clean Lua environment and expose `args`, `cmd`, and headless `g` state.
3. Execute the script.
4. Drain and send queued commands in FIFO order.
5. Dispatch replies and list callbacks in arrival order.
6. Permit callbacks to enqueue additional commands.
7. Continue until there are no queued commands, callbacks, or outstanding requests.
8. Exit zero if all requested work succeeded.
9. If any command fails, report the first/appropriate failures to stderr, finish draining already-issued work, and exit nonzero.

Commands after a destructive operation remain FIFO-ordered. For example, commands after `cmd.server.stop()` are rejected when the Server disappears; commands after removing a target are rejected if that target no longer exists. Those failures are reported according to the normal headless error contract.

## Error behavior

Examples of errors include:

- missing required target in headless mode;
- unknown qualified ID;
- ambiguous or duplicate name during resolution;
- duplicate sibling name on create/rename;
- invalid or empty name;
- a name containing `:`;
- invalid working directory;
- attempting a TUI-only operation headlessly;
- a command targeting an object removed by an earlier command.

TUI behavior should continue to expose Server failures through the existing error state (`g.last_error`) and normal event/state pathways. Headless behavior writes failures to stderr and exits nonzero after draining already-issued work.

## Removal and lifecycle rules

Preserve existing Server invariants:

- Removing a Tab that leaves its Workspace empty closes that Workspace.
- Removing a Workspace that leaves its Session empty closes that Session.
- The Server exits when its final Session closes.
- Removing a Session removes all its Workspaces and Tabs.
- A direct removal operation applies the normal cascading lifecycle behavior.

## Protocol and implementation guidance

The current protocol has Client requests with implicit Session context and Admin requests for global operations. Extend or refactor the protocol so headless requests can carry explicit qualified targets and return typed list data without requiring a Client attachment.

The implementation should preserve the Server as the authority for:

- name uniqueness;
- qualified-ID construction and resolution;
- creation and removal invariants;
- active Workspace/Tab state;
- command ordering and request replies.

Do not implement target resolution solely in Lua or the CLI. The Server must validate every target and name constraint.

Refactor the existing runtime command registration as needed so the same canonical Lua functions can emit either:

- targeted Server requests in headless mode; or
- targeted/implicit Server requests through the TUI event loop in Client mode.

Client-only operations must not leak into the headless command set.

## Acceptance criteria

The implementation is ready when these scenarios work:

1. `zmux run script.lua` executes without opening a TUI and does not execute `zmux.lua`.
2. Script arguments after the file path are available through `args`.
3. `--path` controls default Session/Workspace creation directory.
4. `--socket` selects the intended Server instance.
5. A script can create a Session with its initial Workspace and Tab.
6. A script can create, list, rename, focus, setcwd, and remove Workspaces by qualified ID.
7. A script can create, list, rename, focus, and remove Tabs by qualified ID.
8. `list(callback)` results can be consumed by later commands issued inside the callback.
9. Commands execute FIFO and the process waits for quiescence before exiting.
10. TUI calls without explicit targets use active `g` context.
11. Headless calls without required targets fail.
12. `cmd.session.focus()` fails headlessly and does not claim to change a global active Session.
13. Names containing `:` and duplicate sibling names are rejected.
14. Renaming a parent updates its qualified ID and descendant IDs.
15. Removing the last Tab/Workspace applies the existing cascade.
16. Failed commands produce stderr output and a nonzero headless exit status.
17. Accepted `cmd.server.stop()` exits successfully despite the connection closing.
18. Client/UI commands are unavailable or rejected in headless mode.

## Out of scope for this handoff

- Persistent Sessions across Server restarts.
- Remote or networked scripting.
- A stable compatibility layer for the pre-existing command names.
- Client automation, TUI input injection, rendering, or presentation state.
- Synchronous Lua APIs.
