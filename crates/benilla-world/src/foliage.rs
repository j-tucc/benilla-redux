//! Foliage classification seam (an Elderoth engine patch).
//!
//! Which models sway in the wind is the program's call, not the engine's: the program registers a
//! [`FoliageClassifier`] once, before the world streams, and the clutter builder and the retained
//! static path ask it. Unset, nothing is foliage and nothing is rigid: clutter still carries its
//! bend attribute (geometry only) and no static batch is marked; nothing moves unless the
//! program also replaces `benilla::gfx_hooks::gfx_vertex_offset`.

use std::sync::OnceLock;

/// The program's foliage tests, on an asset path (`World\...\Tree01.m2`).
#[derive(Clone, Copy)]
pub struct FoliageClassifier {
    /// An alpha-tested, non-WMO batch of this model is a leaf card that sways.
    pub leaf: fn(&str) -> bool,
    /// A ground-clutter model that must never bend (stones in a plant table).
    pub rigid: fn(&str) -> bool,
}

static CLASSIFIER: OnceLock<FoliageClassifier> = OnceLock::new();

/// Registers the classifier; the first registration wins. Call before the world streams.
pub fn set_foliage_classifier(classifier: FoliageClassifier) {
    let _ = CLASSIFIER.set(classifier);
}

/// Whether `path`'s alpha-tested batches are swaying leaf cards.
pub(crate) fn leaf(path: &str) -> bool {
    CLASSIFIER.get().is_some_and(|c| (c.leaf)(path))
}

/// Whether the clutter model at `path` stays rigid.
pub(crate) fn rigid(path: &str) -> bool {
    CLASSIFIER.get().is_some_and(|c| (c.rigid)(path))
}
