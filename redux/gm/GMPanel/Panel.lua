-- GM Panel: the window.

GMPANEL_THEME = {
	backdrop = {
		bgFile = "Interface\\DialogFrame\\UI-DialogBox-Background",
		edgeFile = "Interface\\DialogFrame\\UI-DialogBox-Border",
		tile = true, tileSize = 32, edgeSize = 32,
		insets = { left = 11, right = 12, top = 12, bottom = 11 },
	},
	inset = {
		bgFile = "Interface\\Tooltips\\UI-Tooltip-Background",
		edgeFile = "Interface\\Tooltips\\UI-Tooltip-Border",
		tile = true, tileSize = 16, edgeSize = 16,
		insets = { left = 4, right = 4, top = 4, bottom = 4 },
	},
	insetColor = { 0, 0, 0, 0.6 },
	highlight = "Interface\\QuestFrame\\UI-QuestTitleHighlight",
}

local W, H = 660, 460
local ROW_H, ROWS = 16, 11
local QUALITY = { [0] = "9d9d9d", "ffffff", "1eff00", "0070dd", "a335ee", "ff8000", "e6cc80" }
local DATA_FORM = { items = "additem", tele = "tele", creatures = "npcsummon", quests = "questcomplete", spells = "learn" }

local frame
local ui = {}
local state = {
	results = {}, sel = 0, scope = nil,
	cmd = nil, values = {}, names = {},
	picker = nil,      -- index of the picker parameter whose box has focus
	raw = nil,         -- a raw path being edited in the preview
	confirmUntil = 0,
	searchAt = nil,
}

local function Color(hex, text) return "|cff" .. hex .. text .. "|r" end

-- What a picked row puts on the command line: teleports go by name, the rest by ID.
function GMPanel_RowValue(ds, row)
	if ds == "tele" then return row[2] end
	return row[1]
end

local function Inset(parent)
	local f = CreateFrame("Frame", nil, parent)
	f:SetBackdrop(GMPANEL_THEME.inset)
	local c = GMPANEL_THEME.insetColor
	f:SetBackdropColor(c[1], c[2], c[3], c[4])
	f:SetBackdropBorderColor(0.6, 0.6, 0.6, 1)
	return f
end

local function Text(parent, font, justify)
	local t = parent:CreateFontString(nil, "OVERLAY", font or "GameFontHighlightSmall")
	t:SetJustifyH(justify or "LEFT")
	return t
end

local function Button(parent, label, width)
	local b = CreateFrame("Button", nil, parent, "UIPanelButtonTemplate")
	b:SetWidth(width or 80)
	b:SetHeight(20)
	b:SetText(label)
	return b
end

-- Sets a box's text from code. benilla, like the reference, can deliver OnTextChanged a frame
-- later, so the handler compares against the remembered text instead of a flag.
local function SetBoxText(box, text)
	box.expect = text
	box:SetText(text)
end

local function Box(name, parent, width)
	local e = CreateFrame("EditBox", name, parent, "InputBoxTemplate")
	e:SetWidth(width)
	e:SetHeight(20)
	e:SetAutoFocus(false)
	e:SetFontObject(ChatFontNormal)
	return e
end

-- Rows -----------------------------------------------------------------------

local function RowName(r)
	if r.kind == "data" then
		local row = r.row
		if r.ds == "items" then
			return Color(QUALITY[row[3]] or "ffffff", "[" .. row[2] .. "]")
		end
		if r.ds == "spells" and row[3] and row[3] ~= "" then
			return row[2] .. Color("9d9d9d", " (" .. row[3] .. ")")
		end
		return row[2]
	end
	if r.kind == "cmd" then
		local fav = GMPanel_IsFav(r.e.id) and Color("ffd100", "* ") or ""
		local label = fav .. r.e.label
		if GMPanel.level and r.e.sec > GMPanel.level then label = Color("808080", label) end
		return label
	end
	if r.kind == "raw" then return "." .. r.path end
	if r.kind == "recent" then return r.text end
	if r.kind == "lookup" then return Color("66bbff", r.label) end
	return r.label or ""
end

local function RowMid(r)
	if r.kind == "cmd" then return Color("9d9d9d", "." .. r.e.path) end
	if r.kind == "data" then
		local row = r.row
		if r.ds == "items" then return "ilvl " .. row[4] end
		if r.ds == "tele" then return "map " .. row[3] end
		if r.ds == "creatures" then
			local lv = row[3] == row[4] and tostring(row[3]) or (row[3] .. "-" .. row[4])
			return "lvl " .. lv .. ((row[5] and row[5] ~= "") and ("  <" .. row[5] .. ">") or "")
		end
		if r.ds == "quests" and row[3] then return "lvl " .. row[3] end
	end
	return ""
end

local function RowMeta(r)
	if r.kind == "cmd" or r.kind == "raw" then
		local sec = r.kind == "cmd" and r.e.sec or r.sec
		local name = GMPANEL_SEC_NAMES[sec] or tostring(sec)
		if GMPanel.level and sec > GMPanel.level then return Color("ff4040", name) end
		return Color("9d9d9d", name)
	end
	if r.kind == "data" then return Color("9d9d9d", tostring(r.row[1])) end
	if r.kind == "header" and r.more and r.more > 0 then return Color("66bbff", "+" .. r.more .. " more") end
	return ""
end

local function FirstSelectable()
	for i = 1, table.getn(state.results) do
		local r = state.results[i]
		if r.kind ~= "header" then return i end
	end
	return 0
end

function GMPanel_UpdateResults()
	local n = table.getn(state.results)
	FauxScrollFrame_Update(ui.scroll, n, ROWS, ROW_H)
	local offset = FauxScrollFrame_GetOffset(ui.scroll)
	for i = 1, ROWS do
		local b = ui.rows[i]
		local idx = offset + i
		local r = state.results[idx]
		if r then
			b.index = idx
			if r.kind == "header" then
				b.name:SetText(Color("ffd100", r.label))
			else
				b.name:SetText("  " .. RowName(r))
			end
			b.mid:SetText(RowMid(r))
			b.meta:SetText(RowMeta(r))
			if idx == state.sel then b.selTex:Show() else b.selTex:Hide() end
			b:Show()
		else
			b:Hide()
		end
	end
	if n == 0 then ui.empty:Show() else ui.empty:Hide() end
end

local function EnsureVisible()
	local offset = FauxScrollFrame_GetOffset(ui.scroll)
	local target = offset
	if state.sel <= offset then target = state.sel - 1 end
	if state.sel > offset + ROWS then target = state.sel - ROWS end
	if target ~= offset and target >= 0 then
		getglobal(ui.scroll:GetName() .. "ScrollBar"):SetValue(target * ROW_H)
	end
end

local function SetResults(list)
	state.results = list
	state.sel = FirstSelectable()
	getglobal(ui.scroll:GetName() .. "ScrollBar"):SetValue(0)
	GMPanel_UpdateResults()
end

local function MoveSel(delta)
	local n = table.getn(state.results)
	if n == 0 then return end
	local i = state.sel
	for _ = 1, n do
		i = i + delta
		if i < 1 then i = n end
		if i > n then i = 1 end
		if state.results[i].kind ~= "header" then break end
	end
	state.sel = i
	EnsureVisible()
	GMPanel_UpdateResults()
end

-- Search ---------------------------------------------------------------------

local function RunSearch()
	state.searchAt = nil
	if state.picker and state.cmd then
		local p = state.cmd.params[state.picker]
		SetResults(GMPanel_SearchPicker(GMPANEL_PICKER_DATASET[p.t], ui.params[state.picker].box:GetText()))
		ui.scopeText:SetText("Pick: " .. p.label)
		return
	end
	state.lastQuery = ui.search:GetText()
	SetResults(GMPanel_Search(state.lastQuery, GMPanelDB.cat, state.scope))
	if state.scope then
		local d = GMPANEL_DATASET[state.scope]
		ui.scopeText:SetText("In: " .. (d and d.label or (state.scope == "raw" and "Raw" or "Commands")) .. "  (Esc: all)")
	else
		ui.scopeText:SetText("")
	end
end

local function SearchSoon()
	state.searchAt = GetTime() + 0.15
end

-- Form -----------------------------------------------------------------------

local function Values()
	return state.values
end

local function UpdatePreview()
	if state.raw then return end
	if not state.cmd then
		ui.preview:SetText("")
		return
	end
	ui.preview:SetText(GMPanel_Build(state.cmd, Values()))
end

local function UpdateNote()
	local e = state.cmd
	ui.send:SetText("Send")
	state.clearTarget = false
	if not e then
		ui.note:SetText(state.raw and Color("9d9d9d", "Type the arguments, then Enter") or "")
		return
	end
	local text, sev = GMPanel_TargetNote(e, Values())
	local bits = {}
	if sev == "block" then
		tinsert(bits, Color("ff4040", text))
		if text == "Needs a player or no target" then
			ui.send:SetText("Clear target & send")
			state.clearTarget = true
		end
	elseif sev == "warn" then
		tinsert(bits, Color("ffd100", text))
	else
		tinsert(bits, Color("20ff20", text))
	end
	if GMPanel.level and e.sec > GMPanel.level then
		tinsert(bits, Color("ff4040", "needs level " .. e.sec .. " (" .. GMPANEL_SEC_NAMES[e.sec] .. "), you are " .. GMPanel.level))
	end
	if e.perm then tinsert(bits, Color("ff8000", "permanent: writes the world DB")) end
	if e.note then tinsert(bits, Color("9d9d9d", e.note)) end
	ui.note:SetText(table.concat(bits, "   "))
	if GetTime() < state.confirmUntil then
		ui.send:SetText(Color("ff4040", "Confirm"))
	end
end

local function RefreshForm()
	UpdatePreview()
	UpdateNote()
end

local function ToggleLabel(p, val)
	if p.t == "onoff" then return val and "On" or "Off" end
	return val and "Yes" or "No"
end

local function EnumLabel(p, val)
	for i = 1, table.getn(p.opts) do
		if p.opts[i][1] == val then return p.opts[i][2] end
	end
	return tostring(val)
end

function GMPanel_SelectCommand(e, prefill)
	state.cmd = e
	state.raw = nil
	state.values = {}
	state.names = {}
	state.picker = nil
	state.confirmUntil = 0
	local title = e.label .. "  " .. Color("9d9d9d", "." .. e.path)
	ui.formTitle:SetText(title)
	for i = 1, 4 do
		local w = ui.params[i]
		local p = e.params[i]
		w.label:Hide(); w.box:Hide(); w.toggle:Hide()
		if p then
			local val = prefill and prefill[p.k]
			if val == nil then val = p.def end
			if p.t == "onoff" or p.t == "bool01" then
				if val == nil then
					local known = e.toggle and GMPanel.toggles[e.toggle]
					val = not known
				end
			end
			if p.t == "player" and (val == nil) and not p.opt then
				local kind, name = GMPanel_TargetInfo()
				if kind == "player" then val = name end
			end
			state.values[p.k] = val
			w.label:SetText(p.label .. (p.opt and Color("9d9d9d", " (optional)") or ""))
			w.label:Show()
			if p.t == "onoff" or p.t == "bool01" then
				w.toggle:SetText(ToggleLabel(p, val))
				w.toggle:Show()
			elseif p.t == "enum" then
				w.toggle:SetText(EnumLabel(p, val))
				w.toggle:Show()
			else
				local shown = val
				if GMPANEL_PICKER_DATASET[p.t] and prefill and prefill[p.k .. "_name"] then
					shown = prefill[p.k .. "_name"]
					state.names[p.k] = shown
				end
				SetBoxText(w.box, shown and tostring(shown) or "")
				w.box:Show()
			end
		end
	end
	RefreshForm()
end

local function SelectRaw(path)
	state.cmd = nil
	state.raw = path
	state.picker = nil
	ui.formTitle:SetText("Raw  " .. Color("9d9d9d", "." .. path))
	for i = 1, 4 do
		ui.params[i].label:Hide(); ui.params[i].box:Hide(); ui.params[i].toggle:Hide()
	end
	ui.preview:SetText("." .. path .. " ")
	UpdateNote()
	ui.preview:SetFocus()
end

local function FocusParam(i)
	local e = state.cmd
	if not e then return end
	for j = i, table.getn(e.params) do
		local w = ui.params[j]
		if w.box:IsShown() then
			w.box:SetFocus()
			w.box:HighlightText()
			return true
		end
	end
	ui.preview:SetFocus()
	return false
end

-- Sending --------------------------------------------------------------------

local function Danger()
	local e = state.cmd
	if not e or not e.danger then return false end
	if type(e.danger) == "function" then return e.danger(Values()) end
	return true
end

function GMPanel_SendForm()
	local text = ui.preview:GetText()
	if not text or text == "" then return end
	if state.cmd and not state.raw then
		local missing = GMPanel_Missing(state.cmd, Values())
		if missing then
			for i = 1, table.getn(state.cmd.params) do
				if state.cmd.params[i] == missing then FocusParam(i) end
			end
			ui.note:SetText(Color("ff4040", missing.label .. " is required"))
			return
		end
	end
	if Danger() and GetTime() >= state.confirmUntil then
		state.confirmUntil = GetTime() + 3
		UpdateNote()
		ui.note:SetText(Color("ff4040", "Press Enter or Confirm again within 3 s to run this."))
		return
	end
	state.confirmUntil = 0
	if state.clearTarget then ClearTarget() end
	GMPanel_Send(text)
	ui.preview:AddHistoryLine(text)
	RefreshForm()
	GMPanel_RefreshFavs()
end

-- Activating a result ----------------------------------------------------------

local function Pick(r)
	local e = state.cmd
	local i = state.picker
	local p = e.params[i]
	state.values[p.k] = GMPanel_RowValue(r.ds, r.row)
	state.names[p.k] = r.row[2]
	local w = ui.params[i]
	SetBoxText(w.box, r.row[2])
	state.picker = nil
	RefreshForm()
	if not FocusParam(i + 1) then ui.preview:SetFocus() end
	RunSearch()
end

local function Activate(r, fromKey)
	if not r then return end
	if r.kind == "header" then
		if r.scope then
			state.scope = r.scope
			RunSearch()
		end
		return
	end
	if r.pick and state.picker then
		Pick(r)
		return
	end
	if r.kind == "cmd" then
		GMPanel_SelectCommand(r.e)
		if not FocusParam(1) and fromKey and not GMPanel_Missing(r.e, Values()) then
			GMPanel_SendForm()
		end
	elseif r.kind == "raw" then
		SelectRaw(r.path)
	elseif r.kind == "recent" then
		state.cmd = nil
		state.raw = "recent"
		ui.formTitle:SetText("Recent")
		for i = 1, 4 do
			ui.params[i].label:Hide(); ui.params[i].box:Hide(); ui.params[i].toggle:Hide()
		end
		ui.preview:SetText(r.text)
		UpdateNote()
		if fromKey then GMPanel_SendForm() else ui.preview:SetFocus() end
	elseif r.kind == "lookup" then
		GMPanel_Send("." .. GMPANEL_DATASET[r.ds].lookup .. " " .. r.q)
	elseif r.kind == "data" then
		local e = GMPanel.byId[DATA_FORM[r.ds]]
		local pk = e.params[1].k
		local prefill = {}
		prefill[pk] = GMPanel_RowValue(r.ds, r.row)
		prefill[pk .. "_name"] = r.row[2]
		GMPanel_SelectCommand(e, prefill)
		if r.ds == "tele" then
			GMPanel_SendForm()
		elseif not FocusParam(2) then
			ui.preview:SetFocus()
		end
	end
end

-- Favourites strip ---------------------------------------------------------------

function GMPanel_RefreshFavs()
	if not frame then return end
	local x = 0
	for i = 1, 10 do
		local b = ui.favs[i]
		local e = GMPanel.byId[GMPanelDB.favs[i] or ""]
		if e then
			local label = e.label
			if e.toggle then
				local st = GMPanel.toggles[e.toggle]
				if st == true then label = label .. " " .. Color("20ff20", "on")
				elseif st == false then label = label .. " " .. Color("ff4040", "off")
				else label = label .. " " .. Color("9d9d9d", "?") end
			end
			b.entry = e
			b:SetText(label)
			local w = b:GetTextWidth() + 18
			b:SetWidth(w)
			b:ClearAllPoints()
			b:SetPoint("TOPLEFT", frame, "TOPLEFT", 20 + x, -62)
			x = x + w + 2
			if x > W - 40 then b:Hide() else b:Show() end
		else
			b.entry = nil
			b:Hide()
		end
	end
end

local function FavClick(b, button)
	local e = b.entry
	if not e then return end
	if button == "RightButton" then
		GMPanel_ToggleFav(e.id)
		return
	end
	if e.toggle then
		local p = e.params[1]
		local cur = GMPanel.toggles[e.toggle]
		local v = {}
		v[p.k] = not cur
		GMPanel_SelectCommand(e, v)
		GMPanel_SendForm()
		return
	end
	GMPanel_SelectCommand(e)
	if not GMPanel_Missing(e, Values()) and table.getn(e.params) == 0 then
		GMPanel_SendForm()
	else
		FocusParam(1)
	end
end

-- Categories ---------------------------------------------------------------------

local function RefreshCats()
	for i = 1, table.getn(GMPANEL_CATEGORIES) do
		local b = ui.cats[i]
		if GMPANEL_CATEGORIES[i].key == GMPanelDB.cat then
			b.text:SetTextColor(1, 1, 1)
			b.sel:Show()
		else
			b.text:SetTextColor(1, 0.82, 0)
			b.sel:Hide()
		end
	end
end

local function SelectCat(key)
	GMPanelDB.cat = key
	state.scope = nil
	state.picker = nil
	RefreshCats()
	RunSearch()
end

-- Header line ----------------------------------------------------------------------

local function RefreshHeader()
	local kind, name = GMPanel_TargetInfo()
	local who = kind == "self" and "Self" or (name .. (kind == "npc" and " (NPC)" or ""))
	local lvl = GMPanel.level and ("Lv " .. GMPanel.level .. " " .. GMPANEL_SEC_NAMES[GMPanel.level]) or "Lv ?"
	ui.status:SetText(Color("9d9d9d", lvl .. "   Target: ") .. who)
end

function GMPanel_Refresh()
	if not frame or not frame:IsVisible() then return end
	RefreshHeader()
	GMPanel_RefreshFavs()
	UpdateNote()
	if state.cmd and not state.raw then UpdatePreview() end
	GMPanel_UpdateResults()
end

function GMPanel_LogAdd(msg)
	if ui.log then ui.log:AddMessage(msg) end
end

function GMPanel_OnReply(msg, cap)
	if not frame then return end
	if cap.failed then ui.preview:SetTextColor(1, 0.25, 0.25) end
	if string.find(msg, "|H") and frame:IsVisible() and not state.picker then
		-- `.lookup` replies feed the result list when the name tables are missing.
		SearchSoon()
	end
end

function GMPanel_OnSent()
	if ui.preview then ui.preview:SetTextColor(1, 1, 1) end
end

-- Construction -------------------------------------------------------------------

local function SavePosition()
	GMPanelDB.pos = { frame:GetLeft(), frame:GetTop() }
end

local function Build()
	frame = CreateFrame("Frame", "GMPanelFrame", UIParent)
	frame:SetWidth(W)
	frame:SetHeight(H)
	frame:SetFrameStrata("DIALOG")
	frame:SetToplevel(true)
	frame:SetBackdrop(GMPANEL_THEME.backdrop)
	frame:EnableMouse(true)
	frame:SetMovable(true)
	frame:SetClampedToScreen(true)
	frame:RegisterForDrag("LeftButton")
	frame:SetScript("OnDragStart", function() this:StartMoving() end)
	frame:SetScript("OnDragStop", function() this:StopMovingOrSizing(); SavePosition() end)
	frame:Hide()
	tinsert(UISpecialFrames, "GMPanelFrame")

	local title = Text(frame, "GameFontNormal")
	title:SetPoint("TOPLEFT", frame, "TOPLEFT", 22, -18)
	title:SetText("GM Panel")
	ui.status = Text(frame, "GameFontHighlightSmall", "RIGHT")
	ui.status:SetPoint("TOPRIGHT", frame, "TOPRIGHT", -40, -20)

	local close = CreateFrame("Button", "GMPanelCloseButton", frame, "UIPanelCloseButton")
	close:SetPoint("TOPRIGHT", frame, "TOPRIGHT", -6, -6)

	-- Search
	ui.search = Box("GMPanelSearch", frame, 420)
	ui.search:SetPoint("TOPLEFT", frame, "TOPLEFT", 28, -36)
	ui.scopeText = Text(frame, "GameFontDisableSmall")
	ui.scopeText:SetPoint("LEFT", ui.search, "RIGHT", 10, 0)
	ui.search:SetScript("OnTextChanged", function()
		state.picker = nil
		SearchSoon()
	end)
	ui.search:SetScript("OnEnterPressed", function()
		if state.searchAt or state.picker or this:GetText() ~= state.lastQuery then
			state.picker = nil
			RunSearch()
		end
		Activate(state.results[state.sel], true)
	end)
	ui.search:SetScript("OnTabPressed", function() MoveSel(IsShiftKeyDown() and -1 or 1) end)
	ui.search:SetScript("OnEscapePressed", function()
		if state.scope then
			state.scope = nil
			RunSearch()
		elseif this:GetText() ~= "" then
			this:SetText("")
		else
			frame:Hide()
		end
	end)
	ui.search:SetScript("OnEditFocusGained", function()
		state.picker = nil
		this:HighlightText()
		RunSearch()
	end)

	-- Favourites strip
	ui.favs = {}
	for i = 1, 10 do
		local b = Button(frame, "", 60)
		b:RegisterForClicks("LeftButtonUp", "RightButtonUp")
		b:SetScript("OnClick", function() FavClick(this, arg1) end)
		b:SetScript("OnEnter", function()
			if not this.entry then return end
			GameTooltip:SetOwner(this, "ANCHOR_BOTTOMLEFT")
			GameTooltip:AddLine(this.entry.label)
			GameTooltip:AddLine("." .. this.entry.path, 0.6, 0.6, 0.6)
			GameTooltip:AddLine("Right-click to remove from favourites", 0.4, 0.7, 1)
			GameTooltip:Show()
		end)
		b:SetScript("OnLeave", function() GameTooltip:Hide() end)
		ui.favs[i] = b
	end

	-- Categories
	local cats = Inset(frame)
	cats:SetPoint("TOPLEFT", frame, "TOPLEFT", 16, -88)
	cats:SetPoint("BOTTOMLEFT", frame, "BOTTOMLEFT", 16, 16)
	cats:SetWidth(126)
	ui.cats = {}
	for i = 1, table.getn(GMPANEL_CATEGORIES) do
		local c = GMPANEL_CATEGORIES[i]
		local b = CreateFrame("Button", nil, cats)
		b:SetWidth(116)
		b:SetHeight(19)
		b:SetPoint("TOPLEFT", cats, "TOPLEFT", 5, -6 - (i - 1) * 19)
		b.text = Text(b, "GameFontNormalSmall")
		b.text:SetPoint("LEFT", b, "LEFT", 6, 0)
		b.text:SetText(c.label)
		b.sel = b:CreateTexture(nil, "BACKGROUND")
		b.sel:SetTexture(GMPANEL_THEME.highlight)
		b.sel:SetBlendMode("ADD")
		b.sel:SetAllPoints(b)
		b.sel:Hide()
		b:SetHighlightTexture(GMPANEL_THEME.highlight)
		b.key = c.key
		b:SetScript("OnClick", function() SelectCat(this.key) end)
		ui.cats[i] = b
	end

	-- Results
	local res = Inset(frame)
	res:SetPoint("TOPLEFT", frame, "TOPLEFT", 146, -88)
	res:SetPoint("TOPRIGHT", frame, "TOPRIGHT", -16, -88)
	res:SetHeight(ROWS * ROW_H + 12)
	ui.scroll = CreateFrame("ScrollFrame", "GMPanelResultsScroll", res, "FauxScrollFrameTemplate")
	ui.scroll:SetPoint("TOPLEFT", res, "TOPLEFT", 6, -6)
	ui.scroll:SetPoint("BOTTOMRIGHT", res, "BOTTOMRIGHT", -28, 6)
	ui.scroll:SetScript("OnVerticalScroll", function()
		FauxScrollFrame_OnVerticalScroll(ROW_H, GMPanel_UpdateResults)
	end)
	ui.rows = {}
	for i = 1, ROWS do
		local b = CreateFrame("Button", nil, res)
		b:SetHeight(ROW_H)
		b:SetPoint("TOPLEFT", res, "TOPLEFT", 6, -6 - (i - 1) * ROW_H)
		b:SetPoint("RIGHT", res, "RIGHT", -28, 0)
		b:SetHighlightTexture(GMPANEL_THEME.highlight)
		b.selTex = b:CreateTexture(nil, "BACKGROUND")
		b.selTex:SetTexture(GMPANEL_THEME.highlight)
		b.selTex:SetVertexColor(0.4, 0.6, 1)
		b.selTex:SetBlendMode("ADD")
		b.selTex:SetAllPoints(b)
		b.selTex:Hide()
		b.name = Text(b, "GameFontHighlightSmall")
		b.name:SetPoint("LEFT", b, "LEFT", 2, 0)
		b.name:SetWidth(270)
		b.name:SetHeight(ROW_H)
		b.mid = Text(b, "GameFontHighlightSmall")
		b.mid:SetPoint("LEFT", b, "LEFT", 274, 0)
		b.mid:SetWidth(150)
		b.mid:SetHeight(ROW_H)
		b.meta = Text(b, "GameFontHighlightSmall", "RIGHT")
		b.meta:SetPoint("RIGHT", b, "RIGHT", -2, 0)
		b.meta:SetWidth(70)
		b:RegisterForClicks("LeftButtonUp", "RightButtonUp")
		b:EnableMouseWheel(true)
		b:SetScript("OnMouseWheel", function()
			local bar = getglobal("GMPanelResultsScrollScrollBar")
			bar:SetValue(bar:GetValue() - arg1 * ROW_H * 3)
		end)
		b:SetScript("OnClick", function()
			local r = state.results[this.index]
			if arg1 == "RightButton" then
				if r and r.kind == "cmd" then GMPanel_ToggleFav(r.e.id) end
				return
			end
			state.sel = this.index
			Activate(r, false)
			GMPanel_UpdateResults()
		end)
		b:SetScript("OnEnter", function()
			local r = state.results[this.index]
			if not r then return end
			GameTooltip:SetOwner(this, "ANCHOR_RIGHT")
			if r.kind == "data" and r.ds == "items" then
				GameTooltip:SetHyperlink("item:" .. r.row[1] .. ":0:0:0")
			elseif r.kind == "cmd" then
				GameTooltip:AddLine(r.e.label)
				local parts = { "." .. r.e.path }
				for j = 1, table.getn(r.e.params) do
					local p = r.e.params[j]
					tinsert(parts, p.opt and ("[" .. p.label .. "]") or ("<" .. p.label .. ">"))
				end
				GameTooltip:AddLine(table.concat(parts, " "), 0.6, 0.6, 0.6)
				if r.e.note then GameTooltip:AddLine(r.e.note, 0.6, 0.6, 0.6, 1) end
				GameTooltip:AddLine((GMPanel_IsFav(r.e.id) and "Right-click to unpin" or "Right-click to pin to favourites"), 0.4, 0.7, 1)
			else
				GameTooltip:AddLine(RowName(r))
			end
			GameTooltip:Show()
		end)
		b:SetScript("OnLeave", function() GameTooltip:Hide() end)
		ui.rows[i] = b
	end
	ui.empty = Text(res, "GameFontDisable", "CENTER")
	ui.empty:SetPoint("CENTER", res, "CENTER", 0, 0)
	ui.empty:SetText("No matches")

	-- Form
	local form = Inset(frame)
	form:SetPoint("TOPLEFT", res, "BOTTOMLEFT", 0, -2)
	form:SetPoint("TOPRIGHT", res, "BOTTOMRIGHT", 0, -2)
	form:SetHeight(110)
	ui.formTitle = Text(form, "GameFontNormal")
	ui.formTitle:SetPoint("TOPLEFT", form, "TOPLEFT", 10, -8)
	ui.formTitle:SetText("Pick a command or search")
	ui.params = {}
	for i = 1, 4 do
		local col = math.mod(i - 1, 2)
		local rowY = -28 - math.floor((i - 1) / 2) * 24
		local w = {}
		w.label = Text(form, "GameFontNormalSmall", "RIGHT")
		w.label:SetWidth(96)
		w.label:SetPoint("TOPLEFT", form, "TOPLEFT", 6 + col * 246, rowY - 4)
		w.box = Box("GMPanelParam" .. i, form, 136)
		w.box:SetPoint("TOPLEFT", form, "TOPLEFT", 112 + col * 246, rowY)
		w.toggle = Button(form, "", 80)
		w.toggle:SetPoint("TOPLEFT", form, "TOPLEFT", 106 + col * 246, rowY)
		w.label:Hide(); w.box:Hide(); w.toggle:Hide()
		w.index = i
		w.box.slot = i
		w.toggle.slot = i
		w.box:SetScript("OnTextChanged", function()
			if this:GetText() == this.expect or not state.cmd then return end
			this.expect = nil
			local p = state.cmd.params[this.slot]
			if not p then return end
			local txt = this:GetText()
			if GMPANEL_PICKER_DATASET[p.t] then
				-- Typing in a picker searches its table; a bare number is taken as the ID.
				state.picker = this.slot
				state.names[p.k] = nil
				local n = tonumber(txt)
				state.values[p.k] = n
				SearchSoon()
			elseif p.t == "int" or p.t == "num" then
				state.values[p.k] = tonumber(txt) or (txt ~= "" and txt or nil)
			else
				state.values[p.k] = txt
			end
			state.confirmUntil = 0
			RefreshForm()
		end)
		w.box:SetScript("OnEditFocusGained", function()
			if not state.cmd then return end
			local p = state.cmd.params[this.slot]
			if p and GMPANEL_PICKER_DATASET[p.t] then
				state.picker = this.slot
				RunSearch()
			end
		end)
		w.box:SetScript("OnTabPressed", function()
			if state.picker == this.slot and IsShiftKeyDown() then MoveSel(-1) return end
			if state.picker == this.slot and table.getn(state.results) > 0 and state.names[state.cmd.params[this.slot].k] == nil then
				MoveSel(1)
				return
			end
			FocusParam(this.slot + 1)
		end)
		w.box:SetScript("OnEnterPressed", function()
			local p = state.cmd and state.cmd.params[this.slot]
			if p and state.picker == this.slot and state.names[p.k] == nil then
				if state.searchAt then RunSearch() end
				local r = state.results[state.sel]
				if r and r.pick then Pick(r) return end
			end
			GMPanel_SendForm()
		end)
		w.box:SetScript("OnEscapePressed", function() ui.search:SetFocus() end)
		w.toggle:SetScript("OnClick", function()
			local p = state.cmd and state.cmd.params[this.slot]
			if not p then return end
			if p.t == "enum" then
				local cur, nextVal = state.values[p.k], p.opts[1][1]
				for j = 1, table.getn(p.opts) do
					if p.opts[j][1] == cur then nextVal = p.opts[math.mod(j, table.getn(p.opts)) + 1][1] end
				end
				state.values[p.k] = nextVal
				this:SetText(EnumLabel(p, nextVal))
			else
				state.values[p.k] = not state.values[p.k]
				this:SetText(ToggleLabel(p, state.values[p.k]))
			end
			state.confirmUntil = 0
			RefreshForm()
		end)
		ui.params[i] = w
	end

	local prompt = Text(form, "GameFontNormal")
	prompt:SetPoint("BOTTOMLEFT", form, "BOTTOMLEFT", 12, 12)
	prompt:SetText(">")
	ui.preview = Box("GMPanelPreview", form, 330)
	ui.preview:SetPoint("BOTTOMLEFT", form, "BOTTOMLEFT", 30, 8)
	ui.preview:SetMaxLetters(255)
	ui.preview:SetHistoryLines(20)
	ui.preview:SetScript("OnEnterPressed", function() GMPanel_SendForm() end)
	ui.preview:SetScript("OnEscapePressed", function() ui.search:SetFocus() end)
	ui.preview:SetScript("OnTextChanged", function() this:SetTextColor(1, 1, 1) end)
	ui.send = Button(form, "Send", 130)
	ui.send:SetPoint("LEFT", ui.preview, "RIGHT", 8, 0)
	ui.send:SetScript("OnClick", function() GMPanel_SendForm() end)
	ui.note = Text(form, "GameFontHighlightSmall")
	ui.note:SetPoint("BOTTOMLEFT", ui.preview, "TOPLEFT", -18, 4)
	ui.note:SetPoint("RIGHT", form, "RIGHT", -10, 0)

	-- Replies
	local logBox = Inset(frame)
	logBox:SetPoint("TOPLEFT", form, "BOTTOMLEFT", 0, -2)
	logBox:SetPoint("BOTTOMRIGHT", frame, "BOTTOMRIGHT", -16, 16)
	ui.log = CreateFrame("ScrollingMessageFrame", "GMPanelLog", logBox)
	ui.log:SetPoint("TOPLEFT", logBox, "TOPLEFT", 8, -6)
	ui.log:SetPoint("BOTTOMRIGHT", logBox, "BOTTOMRIGHT", -8, 6)
	ui.log:SetFontObject(GameFontHighlightSmall)
	ui.log:SetJustifyH("LEFT")
	ui.log:SetMaxLines(200)
	ui.log:SetFading(false)
	ui.log:EnableMouseWheel(true)
	ui.log:SetScript("OnMouseWheel", function()
		if arg1 > 0 then this:ScrollUp() else this:ScrollDown() end
	end)
	ui.log:SetScript("OnHyperlinkClick", function() SetItemRef(arg1, arg2, arg3) end)

	frame:SetScript("OnUpdate", function()
		if state.searchAt and GetTime() >= state.searchAt then RunSearch() end
		if state.confirmUntil > 0 and GetTime() >= state.confirmUntil then
			state.confirmUntil = 0
			UpdateNote()
		end
	end)
	frame:SetScript("OnShow", function()
		GMPanel_Probe()
		RefreshCats()
		GMPanel_Refresh()
		RunSearch()
	end)
end

local function Place()
	frame:ClearAllPoints()
	local pos = GMPanelDB and GMPanelDB.pos
	if pos and pos[1] and pos[2] then
		frame:SetPoint("TOPLEFT", UIParent, "BOTTOMLEFT", pos[1], pos[2])
	else
		frame:SetPoint("CENTER", UIParent, "CENTER", 0, 40)
	end
end

function GMPanel_Show(text)
	if not frame then Build() Place() end
	frame:Show()
	if text and text ~= "" then
		ui.search:SetText(text)
		RunSearch()
	end
	ui.search:SetFocus()
end

function GMPanel_Toggle()
	if frame and frame:IsVisible() then
		frame:Hide()
	else
		GMPanel_Show()
	end
end

function GMPanel_Slash(msg)
	msg = msg or ""
	if msg ~= "" then
		GMPanel_Show(msg)
	else
		GMPanel_Toggle()
	end
end

-- Shift-clicking an item link while the panel is open puts it into Add item.
local function InsertItemLink(link)
	local _, _, id = string.find(link or "", "item:(%d+)")
	local _, _, name = string.find(link or "", "%[(.-)%]")
	if not id then return false end
	local e = GMPanel.byId.additem
	local v = { item = tonumber(id), item_name = name or id, count = 1 }
	if state.cmd and state.cmd.params[1] and state.cmd.params[1].t == "item" then e = state.cmd end
	GMPanel_SelectCommand(e, v)
	FocusParam(2)
	return true
end

local function WantsLink()
	return frame and frame:IsVisible() and IsShiftKeyDown()
end

local origSetItemRef = SetItemRef
function SetItemRef(link, text, button)
	if WantsLink() and string.find(link or "", "^item:") and InsertItemLink(link .. " " .. (text or "")) then return end
	return origSetItemRef(link, text, button)
end

local origContainerClick = ContainerFrameItemButton_OnClick
function ContainerFrameItemButton_OnClick(button, ignoreModifiers)
	if button == "LeftButton" and not ignoreModifiers and WantsLink() then
		local link = GetContainerItemLink(this:GetParent():GetID(), this:GetID())
		if link and InsertItemLink(link) then return end
	end
	return origContainerClick(button, ignoreModifiers)
end

SLASH_GMPANEL1 = "/gm"
SlashCmdList["GMPANEL"] = GMPanel_Slash

function GMPanel_OnLoaded()
	if frame then Place() end
end
