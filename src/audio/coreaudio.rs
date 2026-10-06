//! The macOS audio backend: CoreAudio's HAL, read into `hal::HalDevice`s.
//! Every HAL call blocks, so each operation runs on `spawn_blocking`.
//! There is no public API for another process's volume or output device,
//! so per-app streams are not supported here.

use super::backend::{AudioBackend, BackendError, Mute, Node};
use super::hal::{self, Controls, HalDevice};
use super::model::{DeviceKind, Snapshot};
use async_trait::async_trait;
use objc2_core_audio::{
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectHasProperty,
    AudioObjectID, AudioObjectPropertyAddress, AudioObjectSetPropertyData,
    kAudioDevicePropertyDeviceUID, kAudioDevicePropertyMute,
    kAudioDevicePropertyStreamConfiguration, kAudioDevicePropertyVolumeScalar,
    kAudioHardwarePropertyDefaultInputDevice, kAudioHardwarePropertyDefaultOutputDevice,
    kAudioHardwarePropertyDevices, kAudioObjectPropertyElementMain, kAudioObjectPropertyName,
    kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeInput,
    kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject,
};
use std::ffi::{c_char, c_void};
use std::ptr::NonNull;
use std::time::Duration;
use tokio::sync::watch;

/// How often `subscribe` re-reads the HAL to notice outside changes.
const POLL: Duration = Duration::from_millis(500);
const SYSTEM: AudioObjectID = kAudioObjectSystemObject as AudioObjectID;
const MAIN: u32 = kAudioObjectPropertyElementMain;
const PER_APP: &str = "per-app audio isn't available on macOS";
const NO_VOLUME: &str = "this device has no volume control";
const NO_MUTE: &str = "this device has no mute control";

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringGetLength(s: *const c_void) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetCString(s: *const c_void, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFRelease(cf: *const c_void);
}
const CF_UTF8: u32 = 0x0800_0100;

fn address(selector: u32, scope: u32, element: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: element,
    }
}

fn has(id: AudioObjectID, addr: AudioObjectPropertyAddress) -> bool {
    // SAFETY: `addr` is a valid address struct for the duration of the call.
    unsafe { AudioObjectHasProperty(id, NonNull::from(&addr)) }
}

fn check(status: i32, what: &str) -> Result<(), BackendError> {
    if status == 0 {
        Ok(())
    } else {
        Err(BackendError::CoreAudio(format!(
            "{what} failed (OSStatus {status})"
        )))
    }
}

/// A fixed-size property (`u32`, `f32`, a pointer as `usize`).
fn get<T: Copy + Default>(
    id: AudioObjectID,
    addr: AudioObjectPropertyAddress,
) -> Result<T, BackendError> {
    let mut value = T::default();
    let mut size = size_of::<T>() as u32;
    // SAFETY: `value` is valid for `size` bytes; the HAL writes at most that
    // and reports how much it wrote in `size`.
    let status = unsafe {
        AudioObjectGetPropertyData(
            id,
            NonNull::from(&addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    };
    check(status, "reading an audio property")?;
    if size as usize != size_of::<T>() {
        return Err(BackendError::CoreAudio(
            "unexpected property size".to_string(),
        ));
    }
    Ok(value)
}

fn set<T: Copy>(
    id: AudioObjectID,
    addr: AudioObjectPropertyAddress,
    value: T,
) -> Result<(), BackendError> {
    // SAFETY: `value` is valid for `size_of::<T>()` bytes for the call.
    let status = unsafe {
        AudioObjectSetPropertyData(
            id,
            NonNull::from(&addr),
            0,
            std::ptr::null(),
            size_of::<T>() as u32,
            NonNull::from(&value).cast(),
        )
    };
    check(status, "changing an audio property")
}

/// A variable-size property, as 8-byte-aligned words (some contain pointers).
fn get_words(
    id: AudioObjectID,
    addr: AudioObjectPropertyAddress,
) -> Result<(Vec<u64>, usize), BackendError> {
    let mut size = 0u32;
    // SAFETY: `addr` and `size` are valid for the call.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            id,
            NonNull::from(&addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        )
    };
    check(status, "sizing an audio property")?;
    let mut words = vec![0u64; (size as usize).div_ceil(8).max(1)];
    // SAFETY: `words` holds at least `size` bytes; the HAL writes at most
    // `size` and updates it to the bytes written.
    let status = unsafe {
        AudioObjectGetPropertyData(
            id,
            NonNull::from(&addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new(words.as_mut_ptr().cast()).expect("vec pointer"),
        )
    };
    check(status, "reading an audio property")?;
    Ok((words, size as usize))
}

fn bytes(words: &[u64], len: usize) -> &[u8] {
    // SAFETY: a `[u64]` is valid to view as bytes; `len` is clamped to it.
    let all = unsafe { std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * 8) };
    &all[..len.min(all.len())]
}

fn u32_at(b: &[u8], offset: usize) -> Option<u32> {
    b.get(offset..offset + 4)
        .map(|s| u32::from_ne_bytes(s.try_into().expect("4 bytes")))
}

/// A CFString property (the HAL returns it retained; we release it).
fn string(id: AudioObjectID, selector: u32) -> Result<String, BackendError> {
    let raw: usize = get(id, address(selector, kAudioObjectPropertyScopeGlobal, MAIN))?;
    let cf = raw as *const c_void;
    if cf.is_null() {
        return Ok(String::new());
    }
    // SAFETY: `cf` is a CFString the HAL handed us with +1 ownership; the
    // buffer is sized by CF for the worst case plus the terminator, and
    // `cf` is released exactly once.
    unsafe {
        let max = CFStringGetMaximumSizeForEncoding(CFStringGetLength(cf), CF_UTF8) + 1;
        let mut buf = vec![0u8; max.max(1) as usize];
        let ok = CFStringGetCString(cf, buf.as_mut_ptr().cast(), buf.len() as isize, CF_UTF8);
        CFRelease(cf);
        if ok == 0 {
            return Err(BackendError::CoreAudio(
                "unreadable device string".to_string(),
            ));
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        Ok(String::from_utf8_lossy(&buf[..end]).into_owned())
    }
}

/// Channels a device has in `scope`, from its `AudioBufferList` stream
/// configuration: `mNumberBuffers` (u32, padded to 8), then 16-byte
/// `AudioBuffer`s starting with `mNumberChannels` (u32).
fn channels(id: AudioObjectID, scope: u32) -> u32 {
    let Ok((words, len)) = get_words(
        id,
        address(kAudioDevicePropertyStreamConfiguration, scope, MAIN),
    ) else {
        return 0;
    };
    let b = bytes(&words, len);
    let buffers = u32_at(b, 0).unwrap_or(0) as usize;
    (0..buffers).filter_map(|i| u32_at(b, 8 + i * 16)).sum()
}

/// The elements carrying `selector` in `scope`: the main element if the
/// device has it there, else every channel that does.
fn elements(id: AudioObjectID, selector: u32, scope: u32, channel_count: u32) -> Vec<u32> {
    if has(id, address(selector, scope, MAIN)) {
        return vec![MAIN];
    }
    (1..=channel_count)
        .filter(|&c| has(id, address(selector, scope, c)))
        .collect()
}

fn controls(id: AudioObjectID, scope: u32, channel_count: u32) -> Controls {
    let volumes: Vec<f32> = elements(id, kAudioDevicePropertyVolumeScalar, scope, channel_count)
        .into_iter()
        .filter_map(|e| get::<f32>(id, address(kAudioDevicePropertyVolumeScalar, scope, e)).ok())
        .collect();
    let muted = elements(id, kAudioDevicePropertyMute, scope, channel_count)
        .first()
        .and_then(|&e| get::<u32>(id, address(kAudioDevicePropertyMute, scope, e)).ok())
        .map(|m| m != 0);
    Controls {
        volume: hal::loudest(&volumes),
        muted,
    }
}

fn read_devices() -> Result<Vec<HalDevice>, BackendError> {
    let (words, len) = get_words(
        SYSTEM,
        address(
            kAudioHardwarePropertyDevices,
            kAudioObjectPropertyScopeGlobal,
            MAIN,
        ),
    )?;
    let b = bytes(&words, len);
    let ids = (0..len / 4).filter_map(|i| u32_at(b, i * 4));
    Ok(ids
        .map(|id| {
            let outs = channels(id, kAudioObjectPropertyScopeOutput);
            let ins = channels(id, kAudioObjectPropertyScopeInput);
            HalDevice {
                id,
                uid: string(id, kAudioDevicePropertyDeviceUID).unwrap_or_default(),
                name: string(id, kAudioObjectPropertyName).unwrap_or_default(),
                output: (outs > 0).then(|| controls(id, kAudioObjectPropertyScopeOutput, outs)),
                input: (ins > 0).then(|| controls(id, kAudioObjectPropertyScopeInput, ins)),
            }
        })
        // A device without a UID can't be saved in settings or targeted.
        .filter(|d| !d.uid.is_empty())
        .collect())
}

fn default_device(selector: u32) -> Option<AudioObjectID> {
    get::<u32>(
        SYSTEM,
        address(selector, kAudioObjectPropertyScopeGlobal, MAIN),
    )
    .ok()
    .filter(|&id| id != 0)
}

fn read_snapshot() -> Result<Snapshot, BackendError> {
    let devices = read_devices()?;
    Ok(hal::build_snapshot(
        &devices,
        default_device(kAudioHardwarePropertyDefaultOutputDevice),
        default_device(kAudioHardwarePropertyDefaultInputDevice),
    ))
}

fn scope_of(kind: DeviceKind) -> u32 {
    match kind {
        DeviceKind::Output => kAudioObjectPropertyScopeOutput,
        DeviceKind::Input => kAudioObjectPropertyScopeInput,
    }
}

/// The device a node names (by UID) and the direction it is used in.
fn resolve(node: &Node) -> Result<(AudioObjectID, u32, u32), BackendError> {
    let (uid, kind) = match node {
        Node::Sink(uid) => (uid, DeviceKind::Output),
        Node::Source(uid) => (uid, DeviceKind::Input),
        Node::SinkInput(_) => return Err(BackendError::Unsupported(PER_APP.to_string())),
    };
    let scope = scope_of(kind);
    let device = read_devices()?
        .into_iter()
        .find(|d| &d.uid == uid)
        .ok_or(BackendError::NoTarget)?;
    let count = channels(device.id, scope);
    if count == 0 {
        return Err(BackendError::NoTarget);
    }
    Ok((device.id, scope, count))
}

fn set_volume_blocking(node: &Node, percent: u16) -> Result<(), BackendError> {
    let (id, scope, count) = resolve(node)?;
    let targets = elements(id, kAudioDevicePropertyVolumeScalar, scope, count);
    if targets.is_empty() {
        return Err(BackendError::Unsupported(NO_VOLUME.to_string()));
    }
    let scalar = hal::percent_to_scalar(percent);
    for e in targets {
        set(
            id,
            address(kAudioDevicePropertyVolumeScalar, scope, e),
            scalar,
        )?;
    }
    Ok(())
}

fn set_mute_blocking(node: &Node, mute: Mute) -> Result<(), BackendError> {
    let (id, scope, count) = resolve(node)?;
    let targets = elements(id, kAudioDevicePropertyMute, scope, count);
    let Some(&first) = targets.first() else {
        return Err(BackendError::Unsupported(NO_MUTE.to_string()));
    };
    let on = match mute {
        Mute::On => true,
        Mute::Off => false,
        Mute::Toggle => get::<u32>(id, address(kAudioDevicePropertyMute, scope, first))? == 0,
    };
    for e in targets {
        set(
            id,
            address(kAudioDevicePropertyMute, scope, e),
            u32::from(on),
        )?;
    }
    Ok(())
}

fn set_default_blocking(kind: DeviceKind, uid: &str) -> Result<(), BackendError> {
    let scope = scope_of(kind);
    let device = read_devices()?
        .into_iter()
        .find(|d| d.uid == uid && channels(d.id, scope) > 0)
        .ok_or(BackendError::NoTarget)?;
    let selector = match kind {
        DeviceKind::Output => kAudioHardwarePropertyDefaultOutputDevice,
        DeviceKind::Input => kAudioHardwarePropertyDefaultInputDevice,
    };
    set(
        SYSTEM,
        address(selector, kAudioObjectPropertyScopeGlobal, MAIN),
        device.id,
    )
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, BackendError> + Send + 'static,
) -> Result<T, BackendError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| BackendError::CoreAudio(format!("audio task failed: {e}")))?
}

/// `tick` is the change channel `subscribe` was given: every successful
/// write bumps it so the action re-reads the real level (a device may round
/// the volume to its own steps, so the plugin's prediction can be off).
#[derive(Default)]
pub struct CoreAudioBackend {
    tick: std::sync::Mutex<Option<watch::Sender<u64>>>,
}

impl CoreAudioBackend {
    fn written<T>(&self, result: Result<T, BackendError>) -> Result<T, BackendError> {
        if result.is_ok()
            && let Some(tx) = self.tick.lock().expect("tick lock").as_ref()
        {
            tx.send_modify(|n| *n = n.wrapping_add(1));
        }
        result
    }
}

#[async_trait]
impl AudioBackend for CoreAudioBackend {
    async fn snapshot(&self) -> Result<Snapshot, BackendError> {
        blocking(read_snapshot).await
    }

    async fn set_volume(&self, node: &Node, percent: u16) -> Result<(), BackendError> {
        let node = node.clone();
        self.written(blocking(move || set_volume_blocking(&node, percent)).await)
    }

    async fn set_mute(&self, node: &Node, mute: Mute) -> Result<(), BackendError> {
        let node = node.clone();
        self.written(blocking(move || set_mute_blocking(&node, mute)).await)
    }

    async fn set_default(&self, kind: DeviceKind, name: &str) -> Result<(), BackendError> {
        let uid = name.to_string();
        self.written(blocking(move || set_default_blocking(kind, &uid)).await)
    }

    async fn move_stream(&self, _stream: u32, _sink: &str) -> Result<(), BackendError> {
        Err(BackendError::Unsupported(PER_APP.to_string()))
    }

    /// Re-reads the HAL every `POLL` and bumps `tx` when anything changed;
    /// stops when every receiver is gone.
    fn subscribe(&self, tx: watch::Sender<u64>) {
        *self.tick.lock().expect("tick lock") = Some(tx.clone());
        tokio::spawn(async move {
            let mut last: Option<Snapshot> = None;
            while !tx.is_closed() {
                if let Ok(now) = blocking(read_snapshot).await
                    && last.as_ref() != Some(&now)
                {
                    last = Some(now);
                    tx.send_modify(|n| *n = n.wrapping_add(1));
                }
                tokio::time::sleep(POLL).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs on the macOS CI runner, which may have no audio device at all:
    /// the HAL must answer without crashing, whatever it reports.
    #[tokio::test]
    async fn coreaudio_snapshot_does_not_panic() {
        match CoreAudioBackend::default().snapshot().await {
            Ok(snap) => {
                for d in snap.sinks.iter().chain(&snap.sources) {
                    assert!(!d.name.is_empty());
                    assert!(d.volume <= 100);
                }
            }
            Err(e) => eprintln!("no HAL snapshot on this machine: {e}"),
        }
    }

    #[tokio::test]
    async fn per_app_audio_is_unsupported() {
        assert!(matches!(
            CoreAudioBackend::default()
                .set_mute(&Node::SinkInput(1), Mute::On)
                .await,
            Err(BackendError::Unsupported(_))
        ));
        assert!(matches!(
            CoreAudioBackend::default().move_stream(1, "x").await,
            Err(BackendError::Unsupported(_))
        ));
    }
}
