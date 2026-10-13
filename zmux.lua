-- User configuration. LuaLS types come from lua/zmux.lua.
opt.cursor_shape = "block"
opt.prefix_key = "<C-Space>"
opt.sequence_timeout_ms = 0

opt.ui.anchors.top.height = 1
opt.ui.anchors.bottom.height = 1
---@type ZmuxBarContentProvider
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
	return string.format(" Session %d · %dx%d ", g.session.number, g.terminal.columns, g.terminal.rows)
end
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
cmd.bind("q", function()
	cmd.quit()
end, { desc = "Detach from the current session" })
-- cmd.bind("<C-q>", function() cmd.quit() end, { desc = "Detach from the current session" })
cmd.bind("c", function()
	cmd.new_tab()
end, { desc = "Create a tab" })
cmd.bind("x", function()
	cmd.close_tab()
end, { desc = "Close the focused tab" })
cmd.bind("n", function()
	cmd.next_tab()
end, { desc = "Focus the next tab" })
cmd.bind("p", function()
	cmd.previous_tab()
end, { desc = "Focus the previous tab" })
for index = 1, 9 do
	local tab_index = index
	cmd.bind(tostring(index), function()
		cmd.select_tab(tab_index)
	end, {
		desc = "Select tab " .. tab_index,
	})
end
cmd.bind("d", function()
	cmd.toggle_workspace_drawer()
end, { desc = "Toggle workspace drawer" })
cmd.bind("wc", function()
	cmd.create_workspace()
end, { desc = "Create a workspace" })
cmd.bind("we", function()
	cmd.edit_workspace()
end, { desc = "Edit the active workspace" })
cmd.bind("tw", function()
	cmd.close_tab()
end, { desc = "Close the focused tab" })
cmd.bind("tn", function()
	cmd.next_tab()
end, { desc = "Focus the next tab" })
cmd.bind("tp", function()
	cmd.previous_tab()
end, { desc = "Focus the previous tab" })

cmd.bind("r", function()
	cmd.redraw()
end, { desc = "Redraw and invalidate the screen cache" })

-- Runtime commands requested by callbacks are applied after the callback returns.
