-- Embedded Lua API defaults. Rust registers low-level primitives; this file
-- composes them into behaviors that user configuration may replace.

local ui = cmd.ui
local style = cmd.style

style.define_theme("default", {
    ["Pty.Border"] = {},
    Bar = { fg = "gray", bg = "black" },
    ["Bar.Tab"] = { fg = "gray" },
    ["Bar.TabActive"] = { fg = "black", bg = "cyan", bold = true },
    ["Bar.Leader"] = { fg = "gray", bg = "blue" },
    ["Bar.LeaderIndicator"] = { fg = "black", bg = "yellow", bold = true },
    ["Bar.Status"] = { fg = "gray", bg = "black" },
    ["Bar.Error"] = { fg = "red", bg = "black" },
    ["Modal.Border"] = {},
    ["Modal.Background"] = { fg = "gray", bg = "black" },
    ["Modal.Query"] = { fg = "yellow", bg = "black" },
    ["Modal.Selected"] = { fg = "black", bg = "cyan", bold = true },
    ["Modal.Item"] = { fg = "white", bg = "black" },
    ["Modal.Footer"] = { fg = "gray", bg = "black" },
    ["Warning.Key"] = { fg = "yellow", bold = true },
    ["Warning.Reload"] = { fg = "yellow", bold = true },
    ["WhichKey.Border"] = { fg = "cyan" },
    ["WhichKey.Background"] = { bg = "black" },
    ["WhichKey.Header"] = { fg = "gray" },
    ["WhichKey.Prefix"] = { fg = "cyan", bold = true },
    ["WhichKey.Footer"] = { fg = "gray" },
    ["WhichKey.Key"] = { fg = "yellow", bold = true },
    ["WhichKey.Description"] = { fg = "white" },
})
style.apply_theme("default")

local function adjacent_index(items, direction)
    if #items < 2 then return nil end
    local current = 1
    for index, item in ipairs(items) do
        if item.selected then
            current = index
            break
        end
    end
    return (current + direction - 1) % #items + 1
end

cmd.tab.next = function()
    local items = g.tabs or {}
    local index = adjacent_index(items, 1)
    if index then cmd.tab.select(items[index].index) end
end
cmd.tab.previous = function()
    local items = g.tabs or {}
    local index = adjacent_index(items, -1)
    if index then cmd.tab.select(items[index].index) end
end
cmd.workspace.next = function()
    local items = g.workspaces or {}
    local index = adjacent_index(items, 1)
    if index then cmd.workspace.select(items[index].id) end
end
cmd.workspace.previous = function()
    local items = g.workspaces or {}
    local index = adjacent_index(items, -1)
    if index then cmd.workspace.select(items[index].id) end
end

local function item_label(item)
    if type(item) == "string" then
        return item, item
    end
    if type(item) ~= "table" then
        error("picker items must be strings or tables")
    end
    local label = item.label or item.text
    if type(label) ~= "string" then
        error("picker table items require a string 'label' or 'text'")
    end
    local value = item.value
    if value == nil then value = item end
    return label, value
end

local function matching_items(items, query)
    local needle = query:lower()
    local matches = {}
    for index, entry in ipairs(items) do
        local candidate = entry.label:lower()
        local position = 1
        local score = 0
        local matched = true
        for character in needle:gmatch(".") do
            local found = candidate:find(character, position, true)
            if not found then
                matched = false
                break
            end
            score = score + found - position
            position = found + 1
        end
        if matched then
            if needle ~= "" and candidate:sub(1, #needle) == needle then
                score = score - #needle
            end
            matches[#matches + 1] = { index = index, score = score }
        end
    end
    table.sort(matches, function(left, right)
        if left.score == right.score then return left.index < right.index end
        return left.score < right.score
    end)
    local indices = {}
    for index, match in ipairs(matches) do indices[index] = match.index end
    return indices
end

local function remove_last_character(text)
    if #text == 0 then return text end
    local index = #text
    while index > 1 do
        local byte = text:byte(index)
        if byte < 128 or byte >= 192 then break end
        index = index - 1
    end
    return text:sub(1, index - 1)
end

function cmd.ui.pick(items, callback)
    assert(type(items) == "table", "picker items must be an array")
    assert(type(callback) == "function", "picker callback must be a function")

    local entries = {}
    for _, item in ipairs(items) do
        local label, value = item_label(item)
        entries[#entries + 1] = { label = label, value = value }
    end
    if #entries == 0 then
        callback(nil)
        return
    end

    local query = ""
    local selected = 1
    local visible = matching_items(entries, query)
    local first_visible = 1
    local max_rows = math.min(8, #entries)
    local query_text = ui.text({ text = "" })
    local row_nodes = {}
    local children = { query_text }
    for index = 1, max_rows do
        row_nodes[index] = ui.text({ text = "" })
        children[#children + 1] = row_nodes[index]
    end
    local footer = ui.text({ text = "" })
    children[#children + 1] = footer

    local function update()
        visible = matching_items(entries, query)
        if #visible == 0 then
            selected = 0
            first_visible = 1
        else
            selected = math.max(1, math.min(selected, #visible))
            if selected < first_visible then first_visible = selected end
            if selected >= first_visible + max_rows then
                first_visible = selected - max_rows + 1
            end
        end

        query_text.set("text", "> " .. query)
        for row = 1, max_rows do
            local item_index = visible[first_visible + row - 1]
            local node = row_nodes[row]
            if item_index then
                local active = item_index == visible[selected]
                node.set("text", (active and "> " or "  ") .. entries[item_index].label)
                node.set("style", cmd.style.ref(active and "Modal.Selected" or "Modal.Item"))
            else
                node.set("text", "")
            end
        end
        footer.set("text", string.format("%d/%d  Enter: select  Esc: cancel", #visible, #entries))
    end

    local picker
    local completed = false
    local function finish(value)
        if completed then return end
        completed = true
        if picker then picker.hide() end
        picker = nil
        callback(value)
    end

    update()
    picker = ui.create({
        type = "modal",
        title = "Choose",
        content = ui.column(children),
        on_key = function(_, key)
            if key.key == "<Esc>" then
                finish(nil)
            elseif key.key == "<CR>" then
                local item_index = visible[selected]
                if item_index then finish(entries[item_index].value) end
            elseif key.key == "<Up>" then
                if #visible > 0 then selected = ((selected - 2) % #visible) + 1 end
                update()
            elseif key.key == "<Down>" then
                if #visible > 0 then selected = (selected % #visible) + 1 end
                update()
            elseif key.key == "<BS>" then
                query = remove_last_character(query)
                selected = 1
                update()
            elseif not key.ctrl and not key.alt and not key.key:match("^<") then
                query = query .. key.key
                selected = 1
                update()
            end
        end,
        on_paste = function(_, text)
            query = query .. text
            selected = 1
            update()
        end,
    })
    picker.show()
end
