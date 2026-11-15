# benilla-forever

benilla-forever is an addon that gives benilla's 1.12 interface the look and layout of the WoW Forever client (Battle.net product `wowf`, 1.60.x). The addon is a near-port: Forever's frames are rebuilt against the 1.12 API (Lua 5.0) using art taken from the player's own Forever install.

What lives where:

- **Committed here** (original code only):
  - `redux/forever-art/`: the art tool.
  - `redux/forever/manifests/`: which atlases and files each phase needs.
  - This README.
- **Never committed** (REDUX.md rule 3):
  - Anything extracted from the install: `benilla-config/forever-extract/`.
  - The generated art: `benilla-config/AddOns/benilla-forever/Art/`, `Atlas.lua`.
  - The addon itself. It is hand-ported from Blizzard code and keeps its own private git history in `benilla-config/AddOns/benilla-forever/`.

## Getting the art

Forever has no `exportInterfaceFiles` command. Instead, a headless job mode added to
[wow.export](https://github.com/Kruithne/wow.export) reads the local CASC store. The patch lives in a local clone, `wow.export-cli` on branch `cli`; see its `cli/README.md`.

1. **Atlas tables, once.** Export `UiTextureAtlas`, `UiTextureAtlasMember` and `UiTextureAtlasElement` as CSV into `benilla-config/forever-extract/`. A CLI job can also list them by `name`, e.g. `DBFilesClient/UiTextureAtlasMember.db2` (written raw).
2. **The job for a manifest:**
   ```
   forever-art job --extract benilla-config/forever-extract \
     --manifest redux/forever/manifests/unitframes.txt \
     --install "C:\Program Files (x86)\World of Warcraft" --log <job.log> --out <job.json>
   ```
3. **Run the export:**
   ```
   wow.export.exe --disable-auto-update --cli <job.json>
   ```
   It exports only the sheets the job lists, refuses jobs over 500 files, and writes `ok <fid> <path>` lines to the log.
4. **Build the addon's art:**
   ```
   forever-art build --extract benilla-config/forever-extract \
     --manifest redux/forever/manifests/unitframes.txt --log <job.log> \
     --addon benilla-config/AddOns/benilla-forever
   ```

Build the tool with `cargo build --release --manifest-path redux/forever-art/Cargo.toml`. It is its own Cargo workspace, outside upstream's `crates/*` and its lock file.

## How `build` writes the art

- **Atlas regions:** each region is cropped into its own texture, sized as benilla needs: each side a power of two from 8 to 512, shrunk if larger. It is padded by repeating its edge pixels and written as RLE TGA.
- **`Atlas.lua`** holds two tables:
  - `BF_ATLAS[name]` = `{ texture, width, height, left, right, top, bottom }`
  - `BF_TEXTURES[path]` has the same shape.
- **In the addon:**
  - `BF_SetAtlas(tex, name, useAtlasSize)` stands in for the modern `Texture:SetAtlas`.
  - `BF_SetTexture(tex, path)` prefers Forever's copy of a 1.12 path.

## Manifests

A manifest has one entry per line:

- `atlas <name>`, or `atlas <prefix>*`
- `file <path.blp>`

`#` starts a comment. Atlas names come from Forever's `Blizzard_*` code; grep the exported `interface/addons` for `SetAtlas(` and `atlas="`.

## Testing

- **The tool:** `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`, all from `redux/forever-art`. The tests round-trip every written TGA through benilla's own `tga_to_rgba` and its device-texture size gate.
- **The addon:**
  - The `addon_harness` probe.
  - A live run on the test account with a scratch `BENILLA_HOME` whose `config.toml` sets `gxWindow = "1"` (windowed, never fullscreen).
  - That run goes through `redux/tools/offscreen.ps1`. The helper keeps the window off every monitor and never activates it, so it covers no one's screen. A minimized window renders at 0×0, which gives a 1×1 `WOW_LIVE_SHOT`; off-screen, the shot is full size.
  - `WOW_PROBE_LUA` that calls `BF_SetAtlas` and checks `Texture:SetTexture`'s 1/nil answer for every generated texture.
