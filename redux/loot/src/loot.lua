-- Loot Window At Cursor (redux/loot/README.md), as WoW Forever's Controls option: with
-- REDUX_LOOT_AT_CURSOR set, the loot window opens with its first row under the mouse, kept on the
-- screen; unset, the stock LootFrame_OnEvent opens it as a left panel. The host (redux-loot) runs
-- this in each in-game UI once LootFrame exists and keeps REDUX_LOOT_AT_CURSOR current. Lua 5.0,
-- against the 1.12 API.

REDUX_LOOT = { stock = LootFrame_OnEvent }

-- The window's drawn width: the 256-wide frame less the art's empty right edge, the right inset
-- of its HitRectInsets (LootFrame.xml).
local EMPTY_RIGHT = 70

-- The first row's centre from the window's top left, in the window's units: LootButton1's own
-- anchor (TOPLEFT 24,-80 in LootFrame.xml) plus half its size.
local function FirstRow()
    local _, _, _, x, y = LootButton1:GetPoint(1)
    x = (x or 24) + LootButton1:GetWidth() / 2
    y = (y or -80) - LootButton1:GetHeight() / 2
    return x, y
end

-- Where the window's top left goes for a cursor at (cx, cy) on a screen sw by sh, all in the
-- window's units: the first row's centre on the cursor, the drawn window kept on the screen.
function ReduxLoot_Place(cx, cy, sw, sh, w, h, rx, ry)
    local left = cx - rx
    local top = cy - ry
    left = math.max(0, math.min(left, sw - w))
    top = math.min(sh, math.max(top, h))
    return left, top
end

-- LOOT_OPENED with the option on: the stock handler's work, with Show at the cursor in place of
-- ShowUIPanel, so the window is no panel and the panels stay where they are. Escape still closes
-- it (UISpecialFrames, below), and LOOT_CLOSED's HideUIPanel ends in frame:Hide() for a frame that
-- holds no panel slot (UIParent.lua).
function ReduxLoot_Open(frame)
    frame.page = 1
    -- GetCursorPosition answers root units and GetScreenWidth/Height UIParent's (the root's
    -- extent over UIParent's scale); the window's points are in its own scale.
    local scale = frame:GetEffectiveScale()
    local cx, cy = GetCursorPosition()
    local ui = UIParent:GetEffectiveScale() / scale
    local sw = GetScreenWidth() * ui
    local sh = GetScreenHeight() * ui
    local rx, ry = FirstRow()
    local left, top = ReduxLoot_Place(cx / scale, cy / scale, sw, sh,
        frame:GetWidth() - EMPTY_RIGHT, frame:GetHeight(), rx, ry)
    frame:ClearAllPoints()
    frame:SetPoint("TOPLEFT", "UIParent", "BOTTOMLEFT", left, top)
    frame:Show()
    frame:Raise()
    if not frame:IsVisible() then
        CloseLoot(1) -- as the stock handler: the window could not open
    end
end

function LootFrame_OnEvent(event)
    if event == "LOOT_OPENED" and REDUX_LOOT_AT_CURSOR == "1" and this == LootFrame
        and GetLeftFrame() ~= LootFrame and GetCenterFrame() ~= LootFrame then
        return ReduxLoot_Open(this)
    end
    return REDUX_LOOT.stock(event)
end

-- Escape closes a window that holds no panel slot through UISpecialFrames (CloseWindows); as a
-- panel it is already closed by then, so the stock path is unchanged.
table.insert(UISpecialFrames, "LootFrame")
