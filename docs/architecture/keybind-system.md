# Keybind System

Global Zmux key bindings are sequences entered after a required prefix key. Focused UI Elements also have direct, single-key Lua callbacks; those bindings are scoped to the focused element and do not require the prefix.

```lua
opt.prefix_key = "<C-Space>"
opt.sequence_timeout_ms = 700

cmd.key.bind("tw", function()
    cmd.tab.close()
end, { desc = "Close the focused tab" })

-- Remove a binding
cmd.key.bind("tw", nil)
```

## Key notation

Literal printable characters are individual keys in a sequence (`"tw"` is `t`, then `w`). Named and modified keys use Neovim notation, for example `<C-c>`, `<M-x>`, `<S-Up>`, `<CR>`, `<Esc>`, `<Space>`, `<Tab>`, `<BS>`, `<Del>`, `<Home>`, `<PageUp>`, and `<F1>`. Modifier keys may be combined, such as `<C-S-Up>`. Plain key case is meaningful (`"t"` differs from `"T"`).

`opt.prefix_key` uses the same notation but must identify exactly one key. `cmd.key.bind` always uses the global prefix. UI Element bindings use the same notation for direct single keys and are active only while that element is focused.

## Sequences and ambiguous bindings

Bindings are stored in a trie. A sequence can be both a complete binding and a prefix of a longer binding. In that case zmux waits up to `opt.sequence_timeout_ms` for another key. If a longer binding completes, its callback runs; if the timeout expires or the next key no longer matches, the shorter binding runs. A prefix with no complete binding is cancelled on timeout. The default sequence timeout is 700 ms; setting it to `0` disables automatic leader/sequence timeouts. With timeouts disabled, completing a binding still exits leader mode. Escape or an unbound key also leaves leader mode; an unbound key additionally displays the yellow warning.

After the prefix and after each sequence key, zmux shows a small which-key overlay when continuations exist. It is anchored to the bottom of the Client view and spans its full width. Entries show the next key and the binding description where available. Escape cancels the current global sequence and is reserved: `cmd.key.bind` rejects any sequence containing `<Esc>`. An unmatched key ends the sequence and produces a short yellow warning at the bottom right.

Global key callbacks run without arguments. A focused UI Element's direct key callback receives `(element, key)`, where `key` contains the normalized notation and modifier/repeat flags. The local binding gets first chance while global leader mode is inactive; an unmatched local key falls through to the global dispatcher, but a key that would otherwise reach the PTY is swallowed. A no-op local callback consumes its key; a key absent from the local map falls through. While global leader mode is active, local UI bindings are ignored and the global dispatcher owns input. Escape blurs a focused element by default, except that it cancels an active global leader sequence; an element may define its own Escape binding. Both global and UI callbacks may issue application commands through `cmd`; callback errors are caught and shown in the warning area.

## Configuration API

- `cmd.key.bind(keys, callback, { desc = string })` adds/replaces a global binding during configuration loading.
- `cmd.key.bind(keys, nil)` removes a global binding.
- `desc` is optional and is used by the which-key overlay.
- `cmd.ui.create({ type = "drawer", keys = { ... } })` registers direct single-key callbacks with that UI Element. Replacing its `keys` table replaces the full local map.
- Runtime commands called by a binding callback are queued and applied after that callback returns, just like commands requested by registered event callbacks.
- `opt.sequence_timeout_ms` defaults to `700`; `opt.prefix_key` defaults to `<C-Space>`.
