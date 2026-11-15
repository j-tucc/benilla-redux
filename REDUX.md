# benilla-redux

benilla-redux is a fork of [benilla](https://github.com/samwhosung/benilla) (`upstream`), kept so
that upstream changes can always be pulled in with a trivial `git merge upstream/main`. Every
change in this repository follows the rules below. They take precedence over upstream's
`docs/METHOD.md` and `docs/CONTRIBUTING.md`, whose 1.12.1-fidelity rules govern upstream benilla,
not redux's own features.

## Remotes

- `origin` is the fork, `j-tucc/benilla-redux`: push here, open pull requests here.
- `upstream` is `samwhosung/benilla`: fetch and merge from it, never push to it.
- Pull upstream with `git fetch upstream && git merge upstream/main`. If that merge stops on a
  conflict, a patch below has grown too big: shrink it while you resolve the conflict.

## Rule 1: redux code lives in redux-owned files

- Redux features go in redux-owned crates and files, never woven into upstream's code. New crates
  go under `redux/`, and redux binaries start the app through `benilla_app::run_with` the way
  `crates/benilla-app/examples/extended_launcher.rs` does.
- Redux settings are the redux crates' own, never new rows in upstream's CVar table, unless a
  recorded patch below says why that can't be avoided.
- Redux docs live in this file and under `redux/`. Upstream's docs stay as upstream ships them.

## Rule 2: changes to upstream files are tiny, recorded patches

A change to a file that upstream owns (anything that is not redux-owned) is allowed only when the
feature cannot hook in from outside. When it can't:

- Make the smallest possible patch: a hook, a `pub`, a plugin slot, a feature flag. Never the
  feature itself.
- Put each patch in its own commit, prefixed `patch(upstream):`, saying what the patch is and
  why redux-owned code can't do it.
- Record it in the patch ledger below, in the same commit.
- Don't reformat, reorder or rename anything in an upstream file. Every changed line is a future
  merge conflict.
- Leave upstream's `Cargo.toml` and `Cargo.lock` dependency resolution as it is unless a patch
  records why it has to change.

## Rule 3: nothing from a Blizzard install is committed

Interface code, art, models, sounds and data extracted from a WoW install (1.12.1 or any modern
client) are for local use only. They live in gitignored paths (`benilla-config/`, `overrides/`)
and are never committed to this repository, which is public. Third-party SDKs with their own
licences (for example the Streamline/DLSS SDK) are also local and gitignored unless their licence
allows redistribution.

## Redux's crates

`redux/` is its own Cargo workspace (`redux/Cargo.toml`, with its own `Cargo.lock`, started as a
copy of the root's), so upstream's root manifest and lockfile stay untouched. Upstream crates are
path dependencies. After an upstream merge, refresh `redux/Cargo.lock` from the root's
(`cp Cargo.lock redux/` and build) so redux builds benilla at upstream's versions.

- `redux/launcher` (`benilla-redux`): the client through `benilla_app::run_with`, with redux's
  plugins on top. It shares the checkout's `WoW` link, `benilla-config/` and `.probe-identity`
  with benilla's own launcher. Build and run from `redux/`:
  `cargo build --release -p benilla-redux`, then `target/release/benilla-redux`.
- `redux/gfx` (`redux-gfx`): graphics, ported from Elderoth's `elderoth-gfx`. Attribution for
  code it carries from other projects is in `redux/THIRD-PARTY.md`.
  - Anti-aliasing and filtering defaults: 8x MSAA and 16x anisotropic filtering, written once
    into a state folder's `config.toml` (benilla's own `gxMultisample` and `anisotropic`), so the
    Video options show and keep the player's later choice. The GPU clamps both.
  - Enhanced effects, each behind a switch, all off by default (Classic draws benilla's image):
    wind, modern fog and haze, sky, grading, bloom, sun shadows with dappled sun, moonlight,
    light shafts, sun rays, sunlit air, ambient occlusion, ground mist, rain wetness,
    firelight, and Forever water with waves; anti-aliasing MSAA, TAA or DLSS. Set them in `benilla-config/redux-gfx.toml`
    (re-read while the client runs): `preset = "enhanced"` turns every effect on with TAA, and
    `gfx_<name> = true/false`, `gfx_shafts = 0|1|2`, `gfx_aa = 0|1|2` (MSAA, TAA, DLSS),
    `gfx_dlss = 0..4` (DLAA to Ultra Performance) and `gfx_fg = true` (Frame Generation) adjust
    it. `REDUX_GFX=enhanced,dlss_q` (or `classic`, or a comma list of effect names) overrides
    the file for one session.
  - Forever water (`gfx_water`, `redux/gfx/src/water.rs`): WoW Forever's look on open water
    (ocean, river, lake) and WMO exterior water. Refraction and depth absorption read a copy of
    the opaque scene taken before the water draws, so MSAA keeps working. The water also gets
    screen-space reflections with a sky fallback, sun and moon glints, caustics, shore foam,
    soft edges against geometry and Snell's window from below. `gfx_waves` adds Gerstner swell on
    ocean and lake. Magma, slime and interior pools keep the stock shader. The surfaces swap onto
    redux's own material from outside the engine, with no upstream patch.
    `REDUX_WATER_DEBUG=1|2|3` paints the copied depth, the water column or the copied colour.
  - DLSS needs the `dlss` feature (`cargo build --release -p benilla-redux --features dlss`)
    and NVIDIA's Streamline SDK extracted to `redux/overrides/streamline/<tag>/` (or
    `overrides/streamline/` beside the binary, or `REDUX_STREAMLINE=<folder>`). Without it a
    DLSS choice draws as TAA. The boot line `STREAMLINE status=…` says what was found.
- `redux/packs` (`redux-packs`): HD texture packs and HD icons, ported from Elderoth's
  `benilla-adapter`. Nothing is committed: both are read from a local `classic-overrides/` folder
  found beside or above the checkout (or the binary): `texture-packs/<pack>/textures/*.dds` (BC7,
  full mips, optional `pack.json` and `index.json`) and `ICONS/<IconName>.tga`.
  `REDUX_TEXTURE_PACKS=<folder>` and `REDUX_HD_ICONS=<folder>|off` point elsewhere.
  - Choose in `benilla-config/redux-textures.toml`: `texture_pack = "<pack name>"` (default
    `"vanilla"`) and `hd_icons = true` (default false). Changing `texture_pack` while the client
    runs switches the pack live (images reload, then the world rebuilds); HD icons follow the file
    on restart. `REDUX_TEXTURE_PACK=<name>` pins a pack for one session. Unattended runs use
    vanilla and stock icons unless the variables name them.
  - The pack's `textures/interface/` art (4x, BC7) replaces the UI textures it holds: frames,
    buttons, icons and the login screens, laid out at the stock sizes. HD icons win over the
    pack's icons when both are on. `pack_ui = false` (or `REDUX_PACK_UI=0`) keeps the stock UI
    under a pack. UI art follows the pack chosen at startup: benilla caches each UI texture for
    the session, so after a live switch the rest of the UI changes on restart.
  - Character skins stay stock (benilla composites them on the CPU from the archives).
  - Boot lines `TEXTURE_PACKS …`, `TEXTURE_PACK on/off …` and `HD_ICONS …` say what was found;
    `REDUX_TEXTURE_PROBE=<seconds>` prints what the pack served by then.
  - F8 (dev builds) opens the texture bug reporter: every texture in view with its source (pack
    file or vanilla); pick one, add a note, and it saves `benilla-config/texture-reports/<stamp>_<name>/`
    (`report.json`, `screenshot.png`) plus a line in `texture-reports/reports.jsonl`.
- `redux/smooth` (`redux-smooth`): smoothed character models, ported from Elderoth's
  `benilla-adapter/src/smooth` (without its chest rig): one level of PN-triangle refinement over
  every skinned part of a `Character\` model, built once per mesh and swapped in from outside
  benilla (no engine patch). Off by default: `smooth_characters = true` in
  `benilla-config/redux-gfx.toml` (followed live), or `REDUX_SMOOTH=1|0` for a session.
  `REDUX_SMOOTH_PROBE=<seconds>` prints how many parts were smoothed and the cost.

Gitignored local files for redux live under `redux/overrides/` (`redux/.gitignore`).

## Tools

- [`redux/headless/README.md`](redux/headless/README.md): run the client with no visible window
  and write screenshots to disk (`python redux/headless/shoot.py`), for agents testing on a
  machine someone is using.

## Features

- **Cursor Size** (`redux/cursor`): the cursor drawn at a chosen size (Default, 32x32 … 128x128)
  or the display's DPI, set in `redux-cursor.toml`, `REDUX_CURSOR_SIZE`, or by an options addon
  through the `reduxCursorSize` CVar. See `redux/cursor/README.md`.
- **GM Panel** (`redux/gm`): `/gm`, a searchable window for vmangos GM commands, as a 1.12 addon
  installed into `benilla-config/AddOns` by `python redux/gm/install.py`. Its item, teleport,
  creature, quest and spell names are generated from the local vmangos DB and never committed.
  See `redux/gm/README.md`.
- **Loot Window At Cursor** (`redux/loot`): WoW Forever's option. When it's on, the loot window
  opens with its first item under the cursor, kept on the screen. Off by default (the stock left
  panel place). Set it in `redux-loot.toml` (`loot_at_cursor = true`) or on the options page. A
  redux-owned Lua hook on `LootFrame_OnEvent`, with no upstream file changes. See
  `redux/loot/README.md`.
- **In-game options** (`redux/options`): a **benilla-redux** group in benilla's Options window
  (Enhanced Graphics, Textures, General) for every setting above, writing the same TOML keys, so
  nothing needs a hand-edited file. Added from outside through the window's Lua tables and row
  templates; no upstream file changes. See `redux/options/README.md`.

## Patch ledger

Every upstream file redux changes, with the reason. Keep it current: a patch with no row here is
a bug.

| Upstream file | Lines changed | Why it can't live in redux-owned code |
|---|---|---|
| `AGENTS.md` | +6 | Points agents at this file before upstream's rules load. |
| `crates/benilla-assets/src/materials.rs`, `crates/benilla-assets/src/shaders/gfx_data.wgsl`, `crates/benilla-assets/src/shaders/gfx_hooks.wgsl`, `crates/benilla-assets/src/shaders/liquid.wgsl`, `crates/benilla-assets/src/shaders/terrain.wgsl`, `crates/benilla-assets/src/shaders/wdl.wgsl`, `crates/benilla-assets/src/shaders/wow_model.wgsl`, `crates/benilla-world/src/clouds/layer.rs`, `crates/benilla-world/src/ffx_glow.rs`, `crates/benilla-world/src/lighting/gfx_data.rs`, `crates/benilla-world/src/lighting/global_light.rs`, `crates/benilla-world/src/lighting/mod.rs`, `crates/benilla-world/src/shaders/cloud.wgsl`, `crates/benilla-world/src/shaders/sky.wgsl`, `crates/benilla-world/src/shaders/static_gx.wgsl`, `crates/benilla-world/src/shaders/wow_effect.wgsl`, `crates/benilla-world/src/sky.rs` | +361/-23 | redux-gfx's fog, wind, sky and cloud bodies run inside the engine's own receiver and dome shaders, and its passes order before the FFX combine; only a no-op hook module at a fixed handle (`materials::GFX_HOOKS_SHADER`), a 64-row `lighting::GfxData` slot, a public `FfxGlowLabel`, an `FfxDither` switch and read-only `WowLighting` accessors. Stock bodies and a zero slot draw benilla's image. |
| `crates/benilla-world/src/clutter.rs`, `crates/benilla-world/src/foliage.rs`, `crates/benilla-world/src/lib.rs`, `crates/benilla-world/src/static_gx/bake.rs`, `crates/benilla-world/src/static_gx/mod.rs` | +80/-2 | The wind hook needs per-vertex inputs only the mesh builders know: clutter `UV_1` (bend height, phase), static leaf-card vertex-word bit 29, and `foliage::set_foliage_classifier`. The stock hook ignores them. |
| `crates/benilla-assets/src/materials.rs`, `crates/benilla-assets/src/shaders/gfx_hooks.wgsl`, `crates/benilla-assets/src/shaders/terrain.wgsl`, `crates/benilla-assets/src/shaders/wow_model.wgsl`, `crates/benilla-assets/src/shaders/wow_model_depth.wgsl`, `crates/benilla-world/src/clouds/layer.rs`, `crates/benilla-world/src/clutter.rs`, `crates/benilla-world/src/lighting/global_light.rs`, `crates/benilla-world/src/lighting/mod.rs`, `crates/benilla-world/src/shaders/static_gx.wgsl`, `crates/benilla-world/src/sky.rs`, `crates/benilla-world/src/static_gx/render.rs`, `crates/benilla-world/src/static_gx/render/shadow.rs`, `crates/benilla-world/src/sun/materials.rs` | +891/-22 | Sun shadows and moonlight need the receivers' light sums (`gfx_lit`, `gfx_baked_aside`) and real casters: the model lane's skinned depth stage (`wow_model_depth.wgsl`) and the retained static pass's depth draw into Bevy's directional shadow views (`static_gx/render/shadow.rs`), all crate-private. Stock benilla spawns no shadow light, so no depth pipeline is queued and the image is unchanged. |
| `crates/benilla-assets/src/shaders/gfx_hooks.wgsl`, `crates/benilla-assets/src/shaders/terrain.wgsl`, `crates/benilla-assets/src/shaders/wow_model.wgsl`, `crates/benilla-world/src/shaders/static_gx.wgsl` | +67/-6 | Rain wetness and firelight need a lit surface's albedo, light and colour, which exist only inside the terrain, model-lane and static-pass fragment shaders. The stock body returns the stock colour. |
| `crates/benilla-world/src/static_gx/render.rs` | +17/-1 | Under DLSS upscaling the world draws into a smaller rectangle (`MainPassResolutionOverride`); the private `StaticGxNode` sets no viewport, so WMOs, doodads and trees would draw at full size. Sets the viewport only when the override is present. |
| `crates/benilla-world/src/boot.rs` | +14/-2 | DLSS needs the Vulkan device created through Streamline's interposer, and Bevy takes a device only through `RenderPlugin::render_creation`, which benilla's own `tuned_default_plugins` builds before `run_with` hands over the app. Adds `boot::RENDER_CREATION`, a slot a launcher fills before boot; unset, nothing changes. |
| `crates/benilla-assets/src/materials.rs`, `crates/benilla-assets/src/shaders/wow_model.wgsl`, `crates/benilla-world/src/shaders/static_gx.wgsl` | +20/-21 | Fix-up of the hook and lit/shadow patches so upstream's own tests stay green untouched: the hook shaders register only where `Assets<Shader>` exists (benilla-assets' loader tests build no shader assets), and the patched lines upstream's source-text tests pin (`let intensity =`, `vertex_light(v.normal, point_lit, vc, v.word)`, `var rgb = base.rgb * in.primary;`) keep their text, with the bake step-aside passed as a private instead of a fifth argument. Same image. |
| `crates/benilla-assets/src/adt.rs`, `crates/benilla-assets/src/lib.rs`, `crates/benilla-assets/src/terrain_source.rs` | +285/-4 | HD texture packs replace each ADT tile's ground layers, which the ADT loader packs inside the asset loader into a private 256² array; only a source asked before packing can supply a BC7 1024²/2048² array. Adds `TerrainLayerSource` and its setter; unset, the loader output is unchanged. |
| `crates/benilla-assets/src/blp.rs`, `crates/benilla-assets/src/lib.rs`, `crates/benilla-assets/src/texture_source.rs`, `crates/benilla-assets/src/world_assets/mod.rs` | +172/-2 | Packs replace M2/WMO and clutter textures, read inside `BlpImageLoader`'s WorldArt lane and `WorldAssets::texture` and pooled by the static pass, so they cannot be swapped from outside. Adds `WorldTextureSource` asked before those reads; unset, unchanged. |
| `crates/benilla-assets/src/adt.rs`, `crates/benilla-assets/src/blp.rs`, `crates/benilla-assets/src/lib.rs`, `crates/benilla-assets/src/terrain_source.rs`, `crates/benilla-assets/src/texture_source.rs`, `crates/benilla-world/src/liquid/surface.rs` | +150/-27 | A pack can be switched while the client runs, so both sources become swappable (`RwLock<Option<…>>`), and the Effect/PointSprite lanes and the liquid frame arrays (engine-internal reads) ask the source too. Unset, unchanged. |
| `crates/benilla-assets/src/spatial_cache.rs`, `crates/benilla-world/src/liquid/mod.rs`, `crates/benilla-world/src/liquid/surface.rs`, `crates/benilla-world/src/static_gx/mod.rs`, `crates/benilla-world/src/static_gx/pool.rs`, `crates/benilla-world/src/terrain_stream.rs`, `crates/benilla-world/src/world_map.rs` | +106/-5 | After a pack switch every texture must be read again, but the streamer, map-scoped caches, static_gx texture pool and liquid frame arrays are private; adds a `WorldReload` message that runs the map-change evictions for the same map, plus read-only `StaticGx::items()` and `SpatialCache::iter()` for the texture reporter. Never sent, nothing changes. |
| `crates/benilla-assets/src/lib.rs`, `crates/benilla-assets/src/texture_source.rs`, `crates/benilla-assets/src/world_assets/mod.rs` | +37/-6 | HD icons replace UI sprites, which `WorldAssets` decodes, uploads and caches privately on the main thread, so nothing outside can swap them. Adds `UiSpriteSource`, asked at the two UI sprite upload sites (`sprite_texture_opened`, `sprite_texture_wrapped`) only, so layout sizes (`sprite_dimensions`) stay stock. Unset, unchanged. |
| `crates/benilla-app/src/quest_markers/mod.rs` | +7 | A bug fix, not a hook: the overhead `!` markers never attached their glow shell's authored alpha (`MatAnim`, 0.4), so the additive shell drew at full strength and whited the gold `!` (and the grey and green ones). The marker parts are spawned privately inside the module. To be reported upstream; drop this row when upstream fixes it. |
