# redux-loot: Loot Window At Cursor

WoW Forever's **Controls → Loot Window At Cursor**: when it's on, the loot window opens with its
first item under the mouse cursor, and is kept on the screen. When it's off (the default, as in
Forever), the window opens where 1.12 puts it, the left panel slot.

## Setting it

- In game: ESC → Options → **benilla-redux** → General → Controls → *Loot Window At Cursor*.
- Or in `benilla-config/redux-loot.toml`: `loot_at_cursor = true`. Off is the key's absence.

Either applies at the next loot: the client re-reads the file every half second.

## How it hooks in

1.12 has no such option. `lootUnderMouse` belongs to later clients and isn't in 1.12's CVar
table. So the hook is redux's own Lua, `src/loot.lua`. `LootPlugin` compiles it in and runs it in
each in-game UI once the stock `LootFrame` exists (so again after a `/reload`). No upstream file
changes.

- It wraps the global `LootFrame_OnEvent`, which the frame's `OnEvent` calls by name. With the
  option on, `LOOT_OPENED` does the stock handler's work (`page = 1`, `CloseLoot(1)` if the
  window can't show) with `Show()` in place of `ShowUIPanel`, as Forever's `LootFrame` does. The
  window is then no panel, so the panels stay where they are. Every other event, and
  `LOOT_OPENED` with the option off, runs the stock handler.
- Position: `GetCursorPosition()` (root units) divided by the window's effective scale, minus
  LootButton1's centre (its own anchor plus half its size), so the first row's centre sits on the
  cursor at any UI scale and resolution. The window's drawn part is clamped to the screen: its
  width less the art's empty right edge (the 70 px right inset of `LootFrame`'s HitRectInsets),
  and its full height, against `GetScreenWidth()`/`GetScreenHeight()` (UIParent's units).
- Closing is stock: `LOOT_CLOSED`'s `HideUIPanel` ends in `frame:Hide()` for a frame in no
  panel slot. `LootFrame` is added to `UISpecialFrames`, so Escape closes it too. As a panel it's
  already closed by then, so that changes nothing when the option is off.
- The host sets the global `REDUX_LOOT_AT_CURSOR` (`"1"`/`"0"`) from the file.

## Test

```bash
cargo test --release --manifest-path redux/Cargo.toml -p redux-loot
```

A live check with the headless tool: `--env WOW_PROBE_CHEST=1` opens a real chest's loot and
`--env "WOW_PROBE_HOVER=WorldFrame"` parks the cursor at the screen's centre. A `--lua` frame
on `LOOT_OPENED` can take a `Screenshot()` and compare `GetCursorPosition()` with LootButton1's
centre.
