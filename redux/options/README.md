# redux-options: redux's settings in the Options window

Every benilla-redux setting can be changed in game: ESC → Options shows a **benilla-redux** group
under benilla's own categories, with three pages.

| Page | Rows | File (in `benilla-config/`) | Applies |
|---|---|---|---|
| Enhanced Graphics | Preset (Classic, Enhanced, Custom), Anti-aliasing (MSAA, TAA, DLSS), DLSS Mode, Frame Generation; Effects: Foliage Wind, Modern Fog, Enhanced Sky, Colour Grading, Bloom, Sun Shadows, Moonlight, Light Shafts (Off, Medium, High), Ambient Occlusion, Ground Mist, Wet Surfaces, Sunlit Air, Sun Rays, Firelight, Fog Light Glow, Forever Water, Water Waves; Models: Smooth Characters | `redux-gfx.toml` | live |
| Textures | Texture Pack (Vanilla or any pack under `classic-overrides/texture-packs`); HD Icons, Pack Interface Art | `redux-textures.toml` | pack live; icons and interface art on restart (the group title says so) |
| General | Display: Cursor Size (Default, 32x32 … 128x128), with WoW Forever's stepper arrows either side of the dropdown; Controls: Loot Window At Cursor | `redux-cursor.toml`, through redux-cursor's `reduxCursorSize` CVar; `redux-loot.toml` | live; loot at the next loot |

The pages write the same keys the redux crates read, so their live reload and their defaults are
unchanged, and an edit to a file made outside the game shows on the page within half a second.

- **Preset** is derived from the switches, never stored on its own: every effect off with MSAA is
  Classic, every effect on with TAA or DLSS is Enhanced, anything else Custom. Choosing Classic or
  Enhanced writes `preset = "classic"`/`"enhanced"` and removes the effect and anti-aliasing keys
  (DLSS's mode and frame generation are kept). Changing one switch writes just that `gfx_*` key,
  and a value the preset already gives removes its key, so the file stays as small as a hand
  written one.
- **Defaults** on the Enhanced Graphics and Textures pages removes the page's keys from the file:
  the code defaults (Classic, vanilla, stock icons) come back. On General it sets Default and
  turns Loot Window At Cursor off.
- DLSS Mode and Frame Generation are greyed unless Anti-aliasing is DLSS; Pack Interface Art is
  greyed under Vanilla.
- `REDUX_GFX`, `REDUX_TEXTURE_PACK`, `REDUX_CURSOR_SIZE` and the other session variables still
  win over the files for their session.

## How it hooks in

benilla's Options window (`crates/benilla-app/assets/ui/OptionsFrame.xml`) keeps its category list
and each page's rows in Lua globals (`BENILLA_OPTIONS_CATEGORY_KEYS`, `BENILLA_OPTIONS_PAGE_ROWS`)
and builds its rows from virtual templates (`BenillaOptionsCheckboxRowTemplate`,
`BenillaOptionsDropdownRowTemplate`, …). `src/page.lua` adds the redux category rows, page bodies,
search heads and rows from outside with those templates and benilla's row functions, so the pages
look, hover, search and refresh exactly like benilla's own. No upstream file changes.

- Rows store into `REDUX_OPT_<key>` globals (benilla's `uvar` rows); a change queues
  `key\tvalue` for the host. The group titles inside a page are rows of their own in the page's
  layout chain; benilla's `Refresh` and `Defaults` are wrapped for these pages so they see the
  controls only, and `UpdateDependencies` adds these pages' greying.
- `src/lib.rs` (`OptionsPlugin`, added by the launcher) runs `page.lua` in each in-game UI once
  `BenillaOptionsFrame` exists (so again after a `/reload`), gives it the pack list, pushes the
  values (`src/model.rs`), drains the queue every 100 ms into the files and re-reads the files
  every 500 ms.

The page is redux's own code against benilla's MIT-licensed window, compiled into the binary;
nothing from a Blizzard install is in it. It was not put in the Forever UI work (`redux/forever`)
because that addon is hand-ported Blizzard code kept out of this repository, and a stock-UI player
would lose the settings with it.

## Test

```bash
cargo test --release --manifest-path redux/Cargo.toml -p redux-options
```

A live check with the headless tool: open the window with `--lua` and
`ShowUIPanel(BenillaOptionsFrame); BenillaOptionsFrame_SelectCategory("ReduxGraphics")`, take a
`Screenshot()`, flip a row (`BenillaOptionsRow_Set(BenillaOptionsFrameContainerBodyReduxGraphicsRowPreset, "1")`)
and read the scratch home's `redux-gfx.toml`.
