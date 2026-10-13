# tmux Daemon and Socket Lifecycle: Primary-Source Notes

## Summary

tmux separates the **server** that owns sessions and PTYs from the **client** that owns the user's attached terminal. The client connects to a local Unix-domain stream socket. On first use, a client command that needs a server starts one; later invocations connect to the same server. Detaching ends the client attachment but leaves the server and sessions running. The default server exits when no sessions remain, not merely because a client detached.

## Findings

### Client/server process model

- The tmux server manages clients, sessions, windows, and panes. The server and each client are separate processes communicating over a socket.
- A client connects first. If the socket is absent or refuses a connection and the command allows startup, clients coordinate through a lock file before starting a server, avoiding a concurrent-start race.
- Normal startup forks and daemonizes. tmux also uses a private socket pair to connect the initiating client to the newly started server; the server listens on its named socket for subsequent clients.
- `-D` starts without daemonizing and disables `exit-empty`; `-N` suppresses automatic server startup. With no command, tmux runs its configured default client command, normally `new-session`.

Sources: [tmux manual — DESCRIPTION and SYNOPSIS](https://man.openbsd.org/tmux.1), [client connection/start lock](https://github.com/tmux/tmux/blob/master/client.c), [server startup and listener](https://github.com/tmux/tmux/blob/master/server.c), [fork/daemon helper](https://github.com/tmux/tmux/blob/master/proc.c), [command entry point and socket path](https://github.com/tmux/tmux/blob/master/tmux.c).

### Socket addressing and isolation

- By default, the socket is under `TMUX_TMPDIR`, or `/tmp` when unset, in a per-UID `tmux-UID` directory, with socket name `default`.
- `-L name` chooses another socket name in that directory. `-S path` specifies a full socket path and takes precedence over `-L`. Different sockets identify independent servers and session sets.
- tmux validates the socket directory and uses private permissions. The XDG Base Directory Specification explicitly designates `$XDG_RUNTIME_DIR` for user-specific runtime objects such as sockets: it must be user-owned, mode `0700`, local, and tied to the login lifetime. If it is unset, applications should use a secure replacement and warn.
- `TMUX` is used by nested tmux invocations to find the current server socket.

Sources: [tmux manual — `-L` and `-S`](https://man.openbsd.org/tmux.1), [socket path construction and directory checks](https://github.com/tmux/tmux/blob/master/tmux.c), [Unix socket creation and permissions](https://github.com/tmux/tmux/blob/master/server.c), [XDG Base Directory Specification — `$XDG_RUNTIME_DIR`](https://specifications.freedesktop.org/basedir-spec/latest/#variables).

### Detach, reattach, and server lifetime

- Detach is a client operation: `detach-client` ends an attachment without destroying its target session.
- `attach-session` attaches to an existing session; `new-session -A` attaches when a requested name exists and otherwise creates it.
- `kill-session` destroys one session and detaches its clients. `kill-server` destroys all server clients and sessions.
- By default, the server stays alive while sessions exist, even with no attached clients. `exit-empty` defaults on, so the server exits once no sessions remain. `exit-unattached` is a separate option for exiting when there are no attached clients.
- This is persistence across detach/disconnect while the server remains alive, not durable recovery across server termination or reboot.

Sources: [tmux manual — CLIENTS AND SESSIONS and session commands](https://man.openbsd.org/tmux.1), [server exit conditions](https://github.com/tmux/tmux/blob/master/server.c), [detach implementation](https://github.com/tmux/tmux/blob/master/cmd-detach-client.c), [client detach handling](https://github.com/tmux/tmux/blob/master/server-client.c), [session destruction](https://github.com/tmux/tmux/blob/master/cmd-kill-session.c), [option defaults](https://github.com/tmux/tmux/blob/master/options-table.c).

## Decisions for zmux

The agreed process ownership model is:

```text
zmux CLI/TUI client ── local IPC ── background zmux server
                                        ├─ numbered sessions
                                        │  └─ workspaces
                                        │      └─ tabs / PTYs / terminal state
                                        └─ attached clients
```

- Normal client startup auto-starts the server if needed. The server always runs in the background; no foreground mode is wanted.
- There is a default per-user server socket and optional named sockets for independent servers. Unix-like systems use `$XDG_RUNTIME_DIR`; if it is missing or unusable, zmux uses a private per-user runtime directory and warns. Windows uses a user-scoped named-pipe endpoint behind the same transport abstraction.
- Detach closes the TUI client while preserving the server and sessions. Stop-server closes the TUI, kills all sessions, and shuts down the server.
- Killing the active session switches all clients attached to it to the next session in creation order. If no session remains, those clients close. The server exits when the last session is destroyed; otherwise it remains alive with no attached clients.
- Sessions receive sequential numbers rather than user-supplied names. Launching without a Session Number creates a new Session; specifying an existing number attaches to that Session.
- Multiple clients may attach. Clients send commands to the server and consume state changes rather than keeping authoritative session copies.
- Sessions do not survive a server restart. Server shutdown ends all sessions and their terminal processes.
- Detach, kill-session, and stop-server actions belong in the Lua configuration as unbound placeholders; the user will choose their keys.
- Each Session contains one or more Workspaces. A Workspace's working directory is the default starting directory for its new Tabs; it changes only through Workspace UI, not when a Tab shell changes its own current directory. A Workspace's default name follows the working directory basename, while an explicit name is user-controlled.
- The Workspace Drawer is a left-side overlay: expanded it takes focus and shows full Workspace names; arrows move a temporary highlight, Enter switches the Session's active Workspace for all attached clients and collapses to a one-character rail, and Escape collapses without switching. Overlay changes do not resize PTYs.

## Agreed implementation details

- Explicit attachment uses `zmux --session N`; an unknown Session Number is an error. A launch without the flag creates a new Session.
- Session Numbers increase monotonically from 1 for one Server lifetime, are not reused, and reset after Server restart.
- A newly created Session starts with one Workspace rooted at the Client's launch working directory and one initial Tab started there; the Workspace name defaults to the directory basename.
- `$XDG_RUNTIME_DIR` is preferred on Unix-like systems. If missing or unusable, use a private per-user directory and warn. The implementation validates restrictive ownership and permissions (mode `0700`) and never places the socket directly in a shared temporary directory.
- Each Session's PTYs use the maximum usable pane rows and columns independently across its attached Clients; per-client rendering clips or pads without independently resizing the shell.
