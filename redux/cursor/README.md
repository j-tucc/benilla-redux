# redux-cursor: Cursor Size

WoW Forever's Options → General → **Cursor Size** for benilla-redux: Default, 32x32, 48x48,
64x64, 96x96, 128x128.

## Why the cursor was tiny

benilla shows the 1.12 cursors (`Interface\Cursor\*.blp`, 32×32) as a hardware cursor
(`crates/benilla-app/src/cursor.rs`). Windows and X11 draw a custom cursor at its pixel size and
ignore the display's DPI scaling, so on a 4K screen at 200% it is half the size of the system
arrow.

## What it does

`CursorSizePlugin` runs after benilla's cursor driver (`PostUpdate`; winit applies cursors in
`Last`). When the window's `CursorIcon` is one benilla set, the plugin swaps in a resize of the same
picture:

- The filter is Catmull-Rom bicubic on premultiplied alpha, so edges stay crisp and the outline gets
  no dark fringe.
- The hotspot moves with the image.
- Each picture and size is resized once and cached.

benilla's driver keeps its own record of what it last set, so it never sees the swap. That means
every state is scaled the same way: point, attack, loot, talk, interact, cast, buy, the `Unable`
variants, a held item or spell, and a `SetCursor` custom picture. A size change rescales the cursor
that's showing straight away.

**Default** is the vanilla 32×32 at the display's scale: 32 px at 100%, 48 px at 150%, 64 px at
200%. The numbered sizes are physical pixels. macOS sizes its `NSCursor`s in points, which are
already DPI-correct, so the plugin does nothing there.

No upstream file is changed.

## Setting it

Use any of these:

- **`redux-cursor.toml`** in benilla's state folder (`benilla-config/`). Write `size = 64` (or
  `"64x64"`), or `size = "default"`. The file is re-read when it changes, so an edit applies while
  the client runs.
- **`REDUX_CURSOR_SIZE=96`**, for one session. It is not saved, and while it is set the file isn't
  re-read.
- **The options-menu hook, the `reduxCursorSize` CVar.** An addon's options row registers it and
  sets it:

  ```lua
  RegisterCVar("reduxCursorSize", "0")      -- once, at load
  local size = GetCVar("reduxCursorSize")   -- "0" (Default), "32", "48", "64", "96", "128"
  SetCVar("reduxCursorSize", "64")          -- the dropdown's OnClick / arrow buttons
  ```

  When redux first sees the CVar, it seeds it with the size in force, so `GetCVar` shows the current
  choice. From then on, redux applies each new value and saves it to `redux-cursor.toml`. The CVar
  is the addon's own registered row, not a row in upstream's CVar table. `prefs::CHOICES` and
  `CursorSize::label` give the dropdown's order and labels.

## Running it

`benilla-cursor` is the client with only this plugin on top. A redux launcher adds it in its
`run_with` closure:

```rust
app.add_plugins(redux_cursor::CursorSizePlugin { state_dir: benilla_app::config_dir() });
```

Checks:

```bash
cargo test --manifest-path redux/cursor/Cargo.toml --lib
```

```bash
cargo clippy --manifest-path redux/cursor/Cargo.toml --all-targets -- -D warnings
```

Debug switches:

- `REDUX_CURSOR_LOG=1` logs every cursor handed to the window, with its size and hotspot.
- `REDUX_CURSOR_DUMP=<dir>` writes each source and its resize as PNGs, with the hotspot marked by a
  red texel. A screen capture leaves the hardware cursor out, so this is how to see the result.
