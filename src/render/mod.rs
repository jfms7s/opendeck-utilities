//! Pure renderers: view models in, touch-strip payloads and key-image SVG
//! data URIs out. `render` owns every view-model type (`LevelView`,
//! `ProfileView`), so dependencies always point feature -> render. Sending
//! frames to the deck is `crate::ui`'s job.

pub mod icons;
pub mod level;
pub mod profile;
pub mod text;
pub mod tile;
