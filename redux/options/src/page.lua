-- benilla-redux's pages in benilla's Options window (redux/options/README.md). The host
-- (redux-options) runs this file in each in-game UI once `BenillaOptionsFrame` exists, then
-- pushes the settings in with ReduxOptions_Push and drains changes with ReduxOptions_TakeWrites.
-- Rows are built from benilla's own row templates and work through its row functions, so the
-- pages look, search, hover and refresh like its own. Lua 5.0, against the 1.12 API.

REDUX_OPTIONS = { writes = {}, controls = {} }

-- The pages, in list order: key, list label.
REDUX_OPTIONS_PAGES = {
    { key = "ReduxGraphics", label = "Enhanced Graphics" },
    { key = "ReduxTextures", label = "Textures" },
    { key = "ReduxGeneral", label = "General" },
}

local BODY = "BenillaOptionsFrameContainerBody"

-- ── Tooltips ──
REDUX_TIP_PRESET =
    "Classic draws the world as the original game does. Enhanced turns on every effect below "
    .. "and anti-aliases temporally; it needs a faster machine.\n\nChanging an effect makes the "
    .. "preset Custom."
REDUX_TIP_AA =
    "MSAA smooths the edges of shapes, as the original game does. TAA blends each frame with "
    .. "the last ones, which smooths edges, leaves and shimmer too, and suits the effects.\n\n"
    .. "DLSS (NVIDIA) needs a DLSS build and the Streamline SDK; without them it draws as TAA."
REDUX_TIP_DLSS =
    "DLAA is DLSS at full size, sharper than TAA. The other modes draw the world smaller and "
    .. "upscale it, Quality the least and Ultra Performance the most, for a faster frame."
REDUX_TIP_FG =
    "DLSS frame generation draws a frame between each two the game draws. It runs only under "
    .. "DLSS, on a GPU that has it."
REDUX_TIP_WIND = "Grass and leaves sway in gusts of a shared wind, and grass parts around bodies."
REDUX_TIP_FOG = "A softer, deeper distance fog that warms toward the sun, a thin haze in the air, "
    .. "and dithering against banding in the sky."
REDUX_TIP_SKY = "A smooth sky gradient, a soft glow around the sun, stars at night, and sunlit clouds."
REDUX_TIP_GRADING = "A gentle colour grade for day and night."
REDUX_TIP_BLOOM = "A soft glow around light brighter than white: lava, lit windows and spells."
REDUX_TIP_SHADOWS = "Trees, buildings, creatures and you cast soft shadows from the sun."
REDUX_TIP_MOONLIGHT = "The moon lights the night with a cool glow, and casts shadows when sun "
    .. "shadows are on."
REDUX_TIP_SHAFTS = "Sunbeams and moonbeams through the air, between trees and buildings. High "
    .. "traces each ray in finer steps."
REDUX_TIP_SSAO = "Soft shade in creases and where things meet: under eaves, at the feet of walls, "
    .. "trees and creatures."
REDUX_TIP_MIST = "Low mist pools over the ground at dawn, dusk and night, and in rain."
REDUX_TIP_WETNESS = "Rain darkens and glosses the ground, buildings and trees, and leaves puddles "
    .. "on flat ground."
REDUX_TIP_SCATTER = "The air holds the sun's light: the sun glows through haze and the sky "
    .. "brightens around it, most at dawn and dusk."
REDUX_TIP_RAYS = "Long beams fanning from the sun through gaps in trees and between buildings."
REDUX_TIP_FIRELIGHT = "Campfires, braziers and lanterns cast a warm, flickering light on the "
    .. "ground, walls and people around them."
REDUX_TIP_SMOOTH = "Character models are drawn with rounder, smoother surfaces."
REDUX_TIP_PACK =
    "The HD texture pack the world is drawn with, from classic-overrides\\texture-packs. "
    .. "Vanilla is the game's own textures.\n\nThe world switches at once; the interface art of "
    .. "a pack follows on restart."
REDUX_TIP_HD_ICONS = "Spell and item icons from classic-overrides\\ICONS in high resolution.\n\n"
    .. "Applies on restart."
REDUX_TIP_PACK_UI = "Frames, buttons and the login screens from the texture pack's interface art, "
    .. "when a pack is chosen.\n\nApplies on restart."
REDUX_TIP_CURSOR = "The size of the mouse cursor. Default is the original 32x32 at the display's "
    .. "scale."
REDUX_TIP_LOOT_AT_CURSOR = "Opens the loot window with its first item under the mouse cursor, "
    .. "kept on the screen. Off, it opens at the left of the screen, as the original game does."

-- ── Host bridge ──

-- A row's store is the global REDUX_OPT_<key>; its change is queued for the host.
function ReduxOptions_Push(key, value)
    setglobal("REDUX_OPT_" .. key, value)
end

function ReduxOptions_Queue(key, value)
    table.insert(REDUX_OPTIONS.writes, key .. "\t" .. value)
end

-- Every change since the last call, one `key\tvalue` per line, or nil.
function ReduxOptions_TakeWrites()
    if table.getn(REDUX_OPTIONS.writes) == 0 then return nil end
    local text = table.concat(REDUX_OPTIONS.writes, "\n")
    REDUX_OPTIONS.writes = {}
    return text
end

-- After a push: redraw the page on show, unless a search owns the rows.
function ReduxOptions_Refresh()
    if not BenillaOptionsFrame:IsVisible() then return end
    local key = BenillaOptionsFrame.selectedCategory
    local search = BenillaOptionsFrame.searchText
    if REDUX_OPTIONS.controls[key] and (not search or search == "") then
        BenillaOptionsPage_Refresh(key)
    end
end

-- The pack list: `names` in order, vanilla first.
function ReduxOptions_SetPacks(names)
    local entries = {}
    for _, name in ipairs(names) do
        local text = name
        if name == "vanilla" then text = "Vanilla" end
        table.insert(entries, { text = text, value = name })
    end
    REDUX_OPTIONS.packRow.entries = entries
end

-- ── Building ──

local function place(name, prev)
    BenillaOptionsSearch_Place(name, prev)
end

-- A page body, its search head and its category row.
local function MakePage(key, label, prevRow, gap)
    local list = BenillaOptionsFrameCategoryList
    local row = CreateFrame("Button", "BenillaOptionsFrameCategoryListRow" .. key, list,
        "BenillaOptionsCategoryRowTemplate")
    row:SetPoint("TOPLEFT", prevRow, "BOTTOMLEFT", 0, gap)
    BenillaOptionsCategoryRow_OnLoad(row, key, label)

    local body = CreateFrame("Frame", BODY .. key, BenillaOptionsFrameContainerBody)
    body:SetPoint("TOPLEFT", BenillaOptionsFrameContainerBody, "TOPLEFT", 0, 0)
    body:SetPoint("BOTTOMRIGHT", BenillaOptionsFrameContainerBody, "BOTTOMRIGHT", 0, 0)
    body:Hide()

    local head = CreateFrame("Button", BODY .. "SearchHead" .. key, BenillaOptionsFrameContainerBody,
        "BenillaOptionsSearchHeadTemplate")
    head.categoryKey = key
    getglobal(head:GetName() .. "Label"):SetText(label)
    head:Hide()

    BENILLA_OPTIONS_PAGE_ROWS[key] = {}
    REDUX_OPTIONS.controls[key] = {}
    table.insert(BENILLA_OPTIONS_CATEGORY_KEYS, key)
    return row
end

-- A group title inside a page: a row of its own in the page's chain, never searched or refreshed.
local function Header(key, rkey, text)
    local f = CreateFrame("Frame", BODY .. key .. rkey, getglobal(BODY .. key))
    f:SetHeight(30)
    local label = f:CreateFontString(f:GetName() .. "Label", "OVERLAY", "GameFontHighlightLarge")
    label:SetPoint("BOTTOMLEFT", f, "BOTTOMLEFT", 7, 2)
    label:SetText(text)
    f.searchTag = ""
    f.isHeader = 1
    table.insert(BENILLA_OPTIONS_PAGE_ROWS[key], rkey)
    return f
end

-- A row on a REDUX_OPT_ global, which the host keeps current.
local function Row(key, rkey, template, store, label, tip, child)
    local row = CreateFrame("Frame", BODY .. key .. rkey, getglobal(BODY .. key), template)
    if getglobal("REDUX_OPT_" .. store) == nil then setglobal("REDUX_OPT_" .. store, "0") end
    BenillaOptionsGlobalRow_OnLoad(row, "REDUX_OPT_" .. store, label, tip, child)
    row.reduxKey = store
    row.applyFunc = function() ReduxOptions_Queue(store, getglobal("REDUX_OPT_" .. store)) end
    table.insert(BENILLA_OPTIONS_PAGE_ROWS[key], rkey)
    table.insert(REDUX_OPTIONS.controls[key], rkey)
    return row
end

local function Check(key, rkey, store, label, tip, child)
    return Row(key, rkey, "BenillaOptionsCheckboxRowTemplate", store, label, tip, child)
end

local function Choice(key, rkey, store, label, tip, entries, child)
    local row = Row(key, rkey, "BenillaOptionsDropdownRowTemplate", store, label, tip, child)
    BenillaOptionsDropdownRow_Setup(row, entries)
    return row
end

-- Lay a page out on benilla's chain, as BenillaOptionsPage_Layout does after a search.
local function Layout(key)
    local prev = nil
    for _, rkey in ipairs(BENILLA_OPTIONS_PAGE_ROWS[key]) do
        local name = BODY .. key .. rkey
        place(name, prev)
        getglobal(name):Show()
        prev = name
    end
end

-- WoW Forever's stepper arrows either side of a dropdown: one entry back or on, no wrap.
local function Steppers(row)
    local dd = getglobal(row:GetName() .. "Dropdown")
    local function arrow(suffix, dir, art)
        local b = CreateFrame("Button", row:GetName() .. suffix, row)
        b:SetWidth(24)
        b:SetHeight(24)
        b:SetNormalTexture("Interface\\Buttons\\UI-SpellbookIcon-" .. art .. "-Up")
        b:SetPushedTexture("Interface\\Buttons\\UI-SpellbookIcon-" .. art .. "-Down")
        b:SetDisabledTexture("Interface\\Buttons\\UI-SpellbookIcon-" .. art .. "-Disabled")
        b:SetHighlightTexture("Interface\\Buttons\\UI-Common-MouseHilight", "ADD")
        b:SetScript("OnClick", function()
            local v = BenillaOptionsRow_Get(row)
            local at = 1
            for i, e in ipairs(row.entries) do
                if e.value == v then at = i end
            end
            local to = at + dir
            if to < 1 or to > table.getn(row.entries) then return end
            PlaySound("igMainMenuOptionCheckBoxOn")
            BenillaOptionsRow_Set(row, row.entries[to].value)
            BenillaOptionsPage_Refresh(BenillaOptionsFrame.selectedCategory)
        end)
        b:SetScript("OnEnter", function() BenillaOptionsRow_Hover(row, 1) end)
        b:SetScript("OnLeave", function() BenillaOptionsRow_Hover(row, nil) end)
        return b
    end
    local back = arrow("Back", -1, "PrevPage")
    back:SetPoint("RIGHT", dd, "LEFT", 16, 2)
    local on = arrow("Forward", 1, "NextPage")
    on:SetPoint("LEFT", dd, "RIGHT", -16, 2)
    row.steppers = { back, on }
end

-- Greys a stepper at its end of the list.
local function UpdateSteppers(row)
    if not row.steppers then return end
    local v = BenillaOptionsRow_Get(row)
    local at, n = nil, table.getn(row.entries)
    for i, e in ipairs(row.entries) do
        if e.value == v then at = i end
    end
    if at and at <= 1 then row.steppers[1]:Disable() else row.steppers[1]:Enable() end
    if at and at >= n then row.steppers[2]:Disable() else row.steppers[2]:Enable() end
end

-- ── The pages ──

local function BuildGraphics(key)
    Header(key, "HeadGraphics", "Enhanced graphics")
    Choice(key, "RowPreset", "preset", "Preset", "REDUX_TIP_PRESET", {
        { text = "Classic", value = "0" },
        { text = "Enhanced", value = "1" },
        { text = "Custom", value = "2" },
    })
    Choice(key, "RowAA", "gfx_aa", "Anti-aliasing", "REDUX_TIP_AA", {
        { text = "MSAA", value = "0" },
        { text = "TAA", value = "1" },
        { text = "DLSS", value = "2" },
    })
    Choice(key, "RowDlss", "gfx_dlss", "DLSS Mode", "REDUX_TIP_DLSS", {
        { text = "DLAA", value = "0" },
        { text = "Quality", value = "1" },
        { text = "Balanced", value = "2" },
        { text = "Performance", value = "3" },
        { text = "Ultra Performance", value = "4" },
    }, 1)
    Check(key, "RowFrameGen", "gfx_fg", "Frame Generation", "REDUX_TIP_FG", 1)

    Header(key, "HeadEffects", "Effects")
    Check(key, "RowWind", "gfx_wind", "Foliage Wind", "REDUX_TIP_WIND")
    Check(key, "RowFog", "gfx_fog", "Modern Fog", "REDUX_TIP_FOG")
    Check(key, "RowSky", "gfx_sky", "Enhanced Sky", "REDUX_TIP_SKY")
    Check(key, "RowGrading", "gfx_grading", "Colour Grading", "REDUX_TIP_GRADING")
    Check(key, "RowBloom", "gfx_bloom", "Bloom", "REDUX_TIP_BLOOM")
    Check(key, "RowShadows", "gfx_shadows", "Sun Shadows", "REDUX_TIP_SHADOWS")
    Check(key, "RowMoonlight", "gfx_moonlight", "Moonlight", "REDUX_TIP_MOONLIGHT")
    Choice(key, "RowShafts", "gfx_shafts", "Light Shafts", "REDUX_TIP_SHAFTS", {
        { text = "Off", value = "0" },
        { text = "Medium", value = "1" },
        { text = "High", value = "2" },
    })
    Check(key, "RowSsao", "gfx_ssao", "Ambient Occlusion", "REDUX_TIP_SSAO")
    Check(key, "RowMist", "gfx_mist", "Ground Mist", "REDUX_TIP_MIST")
    Check(key, "RowWetness", "gfx_wetness", "Wet Surfaces", "REDUX_TIP_WETNESS")
    Check(key, "RowScatter", "gfx_scatter", "Sunlit Air", "REDUX_TIP_SCATTER")
    Check(key, "RowRays", "gfx_rays", "Sun Rays", "REDUX_TIP_RAYS")
    Check(key, "RowFirelight", "gfx_firelight", "Firelight", "REDUX_TIP_FIRELIGHT")

    Header(key, "HeadModels", "Models")
    Check(key, "RowSmooth", "smooth_characters", "Smooth Characters", "REDUX_TIP_SMOOTH")
end

local function BuildTextures(key)
    Header(key, "HeadWorld", "World")
    REDUX_OPTIONS.packRow = Choice(key, "RowPack", "texture_pack", "Texture Pack", "REDUX_TIP_PACK",
        { { text = "Vanilla", value = "vanilla" } })
    Header(key, "HeadInterface", "Interface (applies on restart)")
    Check(key, "RowHdIcons", "hd_icons", "HD Icons", "REDUX_TIP_HD_ICONS")
    Check(key, "RowPackUi", "pack_ui", "Pack Interface Art", "REDUX_TIP_PACK_UI")
end

local function BuildGeneral(key)
    Header(key, "HeadDisplay", "Display")
    -- redux-cursor's own hook (redux/cursor/README.md): the CVar, seeded and saved by the host.
    RegisterCVar("reduxCursorSize", "0")
    local row = CreateFrame("Frame", BODY .. key .. "RowCursorSize", getglobal(BODY .. key),
        "BenillaOptionsDropdownRowTemplate")
    BenillaOptionsRow_OnLoad(row, "reduxCursorSize", "Cursor Size", "REDUX_TIP_CURSOR")
    BenillaOptionsDropdownRow_Setup(row, {
        { text = "Default", value = "0" },
        { text = "32x32", value = "32" },
        { text = "48x48", value = "48" },
        { text = "64x64", value = "64" },
        { text = "96x96", value = "96" },
        { text = "128x128", value = "128" },
    })
    Steppers(row)
    table.insert(BENILLA_OPTIONS_PAGE_ROWS[key], "RowCursorSize")
    table.insert(REDUX_OPTIONS.controls[key], "RowCursorSize")

    -- WoW Forever's Controls > Loot Window At Cursor (redux/loot/README.md).
    Header(key, "HeadControls", "Controls")
    Check(key, "RowLootAtCursor", "loot_at_cursor", "Loot Window At Cursor", "REDUX_TIP_LOOT_AT_CURSOR")
end

-- ── Hooks into benilla's page functions, for these pages only ──

-- Refresh and Defaults walk BENILLA_OPTIONS_PAGE_ROWS[key]; for these pages they see the
-- controls alone, never the group titles.
local function WithControls(key, fn)
    local all = BENILLA_OPTIONS_PAGE_ROWS[key]
    BENILLA_OPTIONS_PAGE_ROWS[key] = REDUX_OPTIONS.controls[key]
    local ok, err = pcall(fn, key)
    BENILLA_OPTIONS_PAGE_ROWS[key] = all
    if not ok then error(err) end
end

local BenillaRefresh = BenillaOptionsPage_Refresh
function BenillaOptionsPage_Refresh(key)
    if not REDUX_OPTIONS.controls[key] then return BenillaRefresh(key) end
    WithControls(key, BenillaRefresh)
end

-- The settings files' defaults are the absence of their keys, which the host writes.
local BenillaDefaults = BenillaOptionsPage_Defaults
function BenillaOptionsPage_Defaults(key)
    if key == "ReduxGraphics" or key == "ReduxTextures" then
        ReduxOptions_Queue("defaults", key)
        return
    end
    if not REDUX_OPTIONS.controls[key] then return BenillaDefaults(key) end
    WithControls(key, BenillaDefaults)
end

local BenillaDependencies = BenillaOptionsPage_UpdateDependencies
function BenillaOptionsPage_UpdateDependencies(key)
    if key == "ReduxGraphics" then
        -- DLSS's mode and frame generation apply only under DLSS.
        local dlss = REDUX_OPT_gfx_aa == "2"
        BenillaOptionsRow_SetEnabled(getglobal(BODY .. key .. "RowDlss"), dlss)
        BenillaOptionsRow_SetEnabled(getglobal(BODY .. key .. "RowFrameGen"), dlss)
        return
    end
    if key == "ReduxTextures" then
        BenillaOptionsRow_SetEnabled(getglobal(BODY .. key .. "RowPackUi"),
            REDUX_OPT_texture_pack ~= "vanilla")
        return
    end
    if key == "ReduxGeneral" then
        UpdateSteppers(getglobal(BODY .. key .. "RowCursorSize"))
        return
    end
    return BenillaDependencies(key)
end

-- ── Install ──

local prev = BenillaOptionsFrameCategoryListRowAudio
local header = CreateFrame("Frame", "BenillaOptionsFrameCategoryListHeaderRedux",
    BenillaOptionsFrameCategoryList, "BenillaOptionsCategoryHeaderTemplate")
header:SetPoint("TOPLEFT", prev, "BOTTOMLEFT", 0, -22)
getglobal(header:GetName() .. "Label"):SetText("benilla-redux")
prev = header
local build = {
    ReduxGraphics = BuildGraphics,
    ReduxTextures = BuildTextures,
    ReduxGeneral = BuildGeneral,
}
for _, page in ipairs(REDUX_OPTIONS_PAGES) do
    prev = MakePage(page.key, page.label, prev, -2)
    build[page.key](page.key)
    Layout(page.key)
end
