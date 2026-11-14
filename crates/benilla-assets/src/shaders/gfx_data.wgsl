#define_import_path benilla::gfx_data

// The program's per-frame graphics data (an Elderoth engine patch): 64 opaque rows appended to the
// shared light buffer right after the point-light table (`lighting::GfxData`, packed by
// `lighting::global_light`). Every receiver's `WowLight` mirror declares it last, as
// `gfx: gfx_data::GfxData`, so no earlier offset moves. The engine reads none of it: only the
// program's `benilla::gfx_hooks` bodies do. All zero is the default.
//
// The block is double the size the program needs today, so it can keep the previous frame's
// rows beside the current ones (for a motion-vector pass) without changing any binding.
struct GfxData {
    rows: array<vec4<f32>, 64>,
}
