-- User configuration. LuaLS types come from lua/zmux.lua.
--
-- The cursor shape used while the PTY is focused: "block", "underline", or
-- "bar".
opt.cursor_shape = "block"
-- Shell executable for newly created sessions. If omitted, $SHELL is used.
opt.default_shell = "/usr/bin/fish"
-- The key that starts a key-binding sequence, in Neovim key notation.
opt.prefix_key = "<C-Space>"
-- Time in milliseconds to wait for the next key in a sequence. Set to zero to
-- keep waiting indefinitely.
opt.sequence_timeout_ms = 0

-- UI anchors reserve rows at the top and bottom of the terminal. Each anchor
-- accepts `height` (rows) and `visible` (boolean).
opt.ui.anchors.top.height = 1
opt.ui.anchors.bottom.height = 1
-- The which-key popup can use at most this many columns. Its default is 7.
opt.ui.which.maxcolumns = 7
-- The which-key popup height can be a percentage string or a row count. The
-- effective value is always clamped to 60% of the terminal height.
opt.ui.which.maxheight = "60%"

-- Bar content may be a string, a segment, a list of segments, or a function
-- returning one of those forms. Functions receive the global `g` state.
---@type ZmuxBarContentProvider
-- Each bar has optional `left`, `center`, and `right` content.
opt.ui.bars.top.left = function(g)
	---@type ZmuxBarSegment[]
	local segments = {}
	for _, tab in ipairs(g.tabs) do
		table.insert(segments, {
			text = string.format(" %d %s ", tab.index, tab.title),
			role = tab.selected and "selected_tab" or "tab",
		})
	end
	return segments
end
opt.ui.bars.bottom.left = function(g)
	if g.last_error then
		return string.format(" Error: %s ", g.last_error)
	end
	if not g.active_tab then
		return " No active tab "
	end
	local status = g.active_tab.running and "running" or "exited"
	return string.format(" %s · %s ", g.active_tab.title, status)
end
opt.ui.bars.bottom.right = function(g)
	return string.format(" Session %s · %dx%d ", g.session.number, g.terminal.columns, g.terminal.rows)
end
-- The default style provider receives the segment and global state. A segment's
-- explicit `style` overrides this provider.
opt.ui.style = function(segment, g)
	if segment.role == "selected_tab" then
		return { fg = "black", bg = "cyan", bold = true }
	end
	if segment.role == "leader" then
		return { fg = "black", bg = "yellow", bold = true }
	end
	return { fg = "gray" }
end

-- Bindings are sequences entered after opt.prefix_key. Descriptions are shown
-- by the which-key popup when a sequence has multiple continuations.
cmd.key.bind("q", function()
	cmd.client.detach()
end, { desc = "Detach from the current session" })
cmd.key.bind("ss", function()
	cmd.server.stop()
end, { desc = "Stop the background server" })
for index = 1, 9 do
	local tab_index = index
	cmd.key.bind(tostring(index), function()
		cmd.tab.select(tab_index)
	end, {
		desc = "Select tab " .. tab_index,
	})
end
cmd.key.bind("d", function()
	cmd.ui.workspace_drawer.toggle()
end, { desc = "Toggle workspace drawer" })
cmd.key.bind("we", function()
	cmd.workspace.edit()
end, { desc = "Edit the active workspace" })
cmd.key.bind("wn", function()
	cmd.workspace.next()
end, { desc = "Focus the next workspace" })
cmd.key.bind("wp", function()
	cmd.workspace.previous()
end, { desc = "Focus the previous workspace" })
cmd.key.bind("wc", function()
	cmd.workspace.create()
end, { desc = "Create a workspace" })
cmd.key.bind("wq", function()
	cmd.workspace.close()
end, { desc = "Close the active workspace" })

cmd.key.bind("tc", function()
	cmd.tab.create()
end, { desc = "Create a tab" })
cmd.key.bind("tw", function()
	cmd.tab.close()
end, { desc = "Close the focused tab" })
cmd.key.bind("n", function()
	cmd.tab.next()
end, { desc = "Focus the next tab" })
cmd.key.bind("p", function()
	cmd.tab.previous()
end, { desc = "Focus the previous tab" })
cmd.key.bind("tc", function()
	cmd.tab.create()
end, { desc = "Create a tab" })
cmd.key.bind("tq", function()
	cmd.tab.close()
end, { desc = "Close the focused tab" })

cmd.key.bind("r", function()
	cmd.ui.redraw()
end, { desc = "Redraw and invalidate the screen cache" })
