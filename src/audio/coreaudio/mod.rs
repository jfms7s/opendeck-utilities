//! The macOS audio backend. `backend.rs` is the only code that calls
//! CoreAudio; `hal.rs` holds its pure conversions, compiled everywhere so
//! Linux tests cover them.

#[cfg(target_os = "macos")]
mod backend;
// Used by the macOS backend; tested on every platform.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod hal;

#[cfg(target_os = "macos")]
pub use backend::CoreAudioBackend;
