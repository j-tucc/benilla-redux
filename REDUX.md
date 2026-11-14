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
    light shafts, sun rays, sunlit air, ambient occlusion, ground mist, rain wetness and
    firelight; anti-aliasing MSAA, TAA or DLSS. Set them in `benilla-config/redux-gfx.toml`
    (re-read while the client runs): `preset = "enhanced"` turns every effect on with TAA, and
    `gfx_<name> = true/false`, `gfx_shafts = 0|1|2`, `gfx_aa = 0|1|2` (MSAA, TAA, DLSS),
    `gfx_dlss = 0..4` (DLAA to Ultra Performance) and `gfx_fg = true` (Frame Generation) adjust
    it. `REDUX_GFX=enhanced,dlss_q` (or `classic`, or a comma list of effect names) overrides
    the file for one session.
  - DLSS needs the `dlss` feature (`cargo build --release -p benilla-redux --features dlss`)
    and NVIDIA's Streamline SDK extracted to `redux/overrides/streamline/<tag>/` (or
    `overrides/streamline/` beside the binary, or `REDUX_STREAMLINE=<folder>`). Without it a
    DLSS choice draws as TAA. The boot line `STREAMLINE status=…` says what was found.

Gitignored local files for redux live under `redux/overrides/` (`redux/.gitignore`).

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
