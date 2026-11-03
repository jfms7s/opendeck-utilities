//! Where platform differences live. Platform-specific helpers are here
//! (backends sit in their own folders: `audio/pactl/` for `pactl`,
//! `audio/coreaudio/` for CoreAudio); logic that must be tested for both
//! platforms takes the platform as a parameter (`MACOS`) instead of being
//! compiled out, so Linux tests cover the macOS branch.

use crate::audio::backend::AudioBackend;
use crate::notifications::NotificationsPort;
use std::path::PathBuf;
use std::sync::Arc;

/// Built for macOS. Pass it to pure functions that branch on the platform.
pub const MACOS: bool = cfg!(target_os = "macos");

/// CoreAudio on macOS, `pactl` (PipeWire/PulseAudio) elsewhere.
pub fn audio_backend() -> Arc<dyn AudioBackend> {
    #[cfg(target_os = "macos")]
    return Arc::new(crate::audio::coreaudio::CoreAudioBackend::default());
    #[cfg(not(target_os = "macos"))]
    return Arc::new(crate::audio::pactl::PactlBackend);
}

/// The desktop's notification inhibition: the session bus on Linux;
/// nothing elsewhere (the Do Not Disturb key says "Linux only").
pub fn notifications() -> Arc<dyn NotificationsPort> {
    #[cfg(target_os = "linux")]
    return crate::notifications::dbus::DbusNotifications::start(None);
    #[cfg(not(target_os = "linux"))]
    return Arc::new(crate::notifications::Unsupported::default());
}

/// The executable of the process that started the plugin (normally OpenDeck).
#[cfg(target_os = "linux")]
pub fn parent_exe() -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{}/exe", std::os::unix::process::parent_id())).ok()
}

/// macOS has no `/proc`; `proc_pidpath` gives the same answer.
#[cfg(target_os = "macos")]
pub fn parent_exe() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: `buf` is valid for `buf.len()` bytes, which is what we pass.
    let len = unsafe {
        libc::proc_pidpath(
            std::os::unix::process::parent_id() as libc::c_int,
            buf.as_mut_ptr().cast(),
            buf.len() as u32,
        )
    };
    if len <= 0 {
        return None;
    }
    buf.truncate(len as usize);
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&buf)))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn parent_exe() -> Option<PathBuf> {
    None
}
