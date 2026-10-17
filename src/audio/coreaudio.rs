//! The macOS audio backend: CoreAudio's HAL, read into `hal::HalDevice`s.
//! Every HAL call blocks, so each operation runs on `spawn_blocking`.
//! There is no public API for another process's volume or output device,
//! so per-app streams are not supported here.

use super::backend::{AudioBackend, BackendError, Mute, Node};
use super::hal::{self, Controls, HalDevice};
use super::model::{DeviceKind, Snapshot};
use async_trait::async_trait;
use hal::POLL;
use objc2_core_audio::{
    AudioObjectAddPropertyListener, AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize,
    AudioObjectHasProperty, AudioObjectID, AudioObjectPropertyAddress, AudioObjectSetPropertyData,
    kAudioDevicePropertyDeviceUID, kAudioDevicePropertyMute,
    kAudioDevicePropertyStreamConfiguration, kAudioDevicePropertyVolumeScalar,
    kAudioHardwarePropertyDefaultInputDevice, kAudioHardwarePropertyDefaultOutputDevice,
    kAudioHardwarePropertyDevices, kAudioObjectPropertyElementMain, kAudioObjectPropertyName,
    kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeInput,
    kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject,
};
use std::ffi::{c_char, c_void};
use std::ptr::NonNull;
use std::sync::Mutex;
use tokio::sync::{Notify, watch};

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
/// `kAudioHardwarePropertyRunLoop` (`'rnlp'`, in objc2-core-audio's
/// `AudioHardwareDeprecated` feature, which this one constant isn't worth).
const RUN_LOOP: u32 = 0x726e_6c70;

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

fn device_ids() -> Result<Vec<AudioObjectID>, BackendError> {
    let (words, len) = get_words(
        SYSTEM,
        address(
            kAudioHardwarePropertyDevices,
            kAudioObjectPropertyScopeGlobal,
            MAIN,
        ),
    )?;
    let b = bytes(&words, len);
    Ok((0..len / 4).filter_map(|i| u32_at(b, i * 4)).collect())
}

fn read_devices() -> Result<Vec<HalDevice>, BackendError> {
    Ok(device_ids()?
        .into_iter()
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

/// What a property listener needs, leaked once per `subscribe` (the
/// plugin's lifetime) so the HAL can hold a plain pointer to it.
struct Listener {
    /// The `subscribe` channel: every callback bumps it.
    tx: watch::Sender<u64>,
    /// Wakes `subscribe`'s loop to give devices that appeared their
    /// listeners (the callback itself makes no HAL calls).
    resync: Notify,
    listened: Mutex<hal::Listened>,
}

/// Called by the HAL on its own thread with the addresses that changed.
unsafe extern "C-unwind" fn on_change(
    _id: AudioObjectID,
    count: u32,
    addresses: NonNull<AudioObjectPropertyAddress>,
    client: *mut c_void,
) -> i32 {
    // SAFETY: `client` is the `&'static Listener` passed at registration
    // (leaked, never freed), and the HAL passes `count` valid addresses
    // that live for the duration of the call.
    let (listener, changed) = unsafe {
        (
            &*client.cast::<Listener>(),
            std::slice::from_raw_parts(addresses.as_ptr(), count as usize),
        )
    };
    listener.tx.send_modify(|n| *n = n.wrapping_add(1));
    if hal::device_list_changed(changed.iter().map(|a| a.mSelector)) {
        listener.resync.notify_one();
    }
    0
}

fn listen(
    id: AudioObjectID,
    addr: AudioObjectPropertyAddress,
    listener: &'static Listener,
) -> Result<(), BackendError> {
    // SAFETY: `addr` is valid for the call (the HAL copies it); `on_change`
    // matches the listener proc type; the client data is a `'static`
    // `Listener` that outlives every callback, and `on_change` only reads it.
    let status = unsafe {
        AudioObjectAddPropertyListener(
            id,
            NonNull::from(&addr),
            Some(on_change),
            std::ptr::from_ref(listener).cast_mut().cast(),
        )
    };
    check(status, "registering an audio listener")
}

/// The system object's device list and both defaults.
fn system_addresses() -> [AudioObjectPropertyAddress; 3] {
    [
        kAudioHardwarePropertyDevices,
        kAudioHardwarePropertyDefaultOutputDevice,
        kAudioHardwarePropertyDefaultInputDevice,
    ]
    .map(|selector| address(selector, kAudioObjectPropertyScopeGlobal, MAIN))
}

/// Volume and mute on every element a device really has, per scope.
fn device_addresses(id: AudioObjectID) -> Vec<AudioObjectPropertyAddress> {
    let mut out = Vec::new();
    for scope in [
        kAudioObjectPropertyScopeOutput,
        kAudioObjectPropertyScopeInput,
    ] {
        let count = channels(id, scope);
        if count == 0 {
            continue;
        }
        for selector in [kAudioDevicePropertyVolumeScalar, kAudioDevicePropertyMute] {
            out.extend(
                elements(id, selector, scope, count)
                    .into_iter()
                    .map(|e| address(selector, scope, e)),
            );
        }
    }
    out
}

/// Registers on the system object. First asks the HAL to call listeners on
/// its own thread: the plugin never runs a main run loop, on which the HAL
/// would otherwise deliver them. That setting failing is not fatal.
fn listen_system(listener: &'static Listener) -> Result<(), BackendError> {
    if let Err(e) = set(
        SYSTEM,
        address(RUN_LOOP, kAudioObjectPropertyScopeGlobal, MAIN),
        std::ptr::null::<c_void>(),
    ) {
        log::debug!("can't detach CoreAudio notifications from the run loop: {e}");
    }
    system_addresses()
        .into_iter()
        .try_for_each(|a| listen(SYSTEM, a, listener))
}

/// Gives every device without listeners its own. A failed device is
/// forgotten, so the next sync retries it.
fn listen_devices(listener: &'static Listener) -> Result<(), BackendError> {
    let ids = device_ids()?;
    let new = listener
        .listened
        .lock()
        .expect("listened lock")
        .new_devices(&ids);
    let mut result = Ok(());
    for id in new {
        if let Err(e) = device_addresses(id)
            .into_iter()
            .try_for_each(|a| listen(id, a, listener))
        {
            listener.listened.lock().expect("listened lock").forget(id);
            result = Err(e);
        }
    }
    result
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

    /// Outside changes arrive through CoreAudio property listeners, which
    /// bump `tx` themselves. This loop gives new devices their listeners
    /// and re-reads the HAL only every `hal::SAFETY_POLL` as a safety net,
    /// or every `POLL` while registration fails. Stops when every receiver
    /// is gone.
    fn subscribe(&self, tx: watch::Sender<u64>) {
        *self.tick.lock().expect("tick lock") = Some(tx.clone());
        let listener: &'static Listener = Box::leak(Box::new(Listener {
            tx: tx.clone(),
            resync: Notify::new(),
            listened: Mutex::new(hal::Listened::default()),
        }));
        tokio::spawn(async move {
            let system = blocking(move || listen_system(listener)).await;
            let system_ok = system.is_ok();
            let mut health = hal::ListenHealth::default();
            let mut last: Option<Snapshot> = None;
            loop {
                let pass = blocking(move || {
                    let synced = if system_ok {
                        listen_devices(listener)
                    } else {
                        Ok(())
                    };
                    Ok((synced, read_snapshot()))
                })
                .await;
                let (synced, now) = match pass {
                    Ok((synced, now)) => (synced, now.ok()),
                    Err(e) => (Err(e), None),
                };
                let listening = system_ok && synced.is_ok();
                if health.report(listening) {
                    let reason = match (&system, &synced) {
                        (Err(e), _) | (_, Err(e)) => e.to_string(),
                        _ => String::new(),
                    };
                    log::warn!(
                        "CoreAudio listeners unavailable ({reason}); re-reading the HAL every {POLL:?}"
                    );
                }
                if let Some(now) = now
                    && last.as_ref() != Some(&now)
                {
                    last = Some(now);
                    tx.send_modify(|n| *n = n.wrapping_add(1));
                }
                tokio::select! {
                    _ = tx.closed() => break,
                    _ = listener.resync.notified() => {}
                    _ = tokio::time::sleep(health.poll_interval()) => {}
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_audio::AudioObjectRemovePropertyListener;

    #[test]
    fn the_device_list_selector_matches_the_hal() {
        assert_eq!(hal::DEVICES_SELECTOR, kAudioHardwarePropertyDevices);
    }

    /// Needs no audio device: the system object always exists.
    #[test]
    fn a_system_listener_registers_and_unregisters() {
        let listener: &'static Listener = Box::leak(Box::new(Listener {
            tx: watch::channel(0).0,
            resync: Notify::new(),
            listened: Mutex::new(hal::Listened::default()),
        }));
        let mut addr = address(
            kAudioHardwarePropertyDefaultOutputDevice,
            kAudioObjectPropertyScopeGlobal,
            MAIN,
        );
        listen(SYSTEM, addr, listener).expect("registering on the system object");
        // SAFETY: the same address, proc and client data as registered above.
        let status = unsafe {
            AudioObjectRemovePropertyListener(
                SYSTEM,
                NonNull::from(&mut addr),
                Some(on_change),
                std::ptr::from_ref(listener).cast_mut().cast(),
            )
        };
        assert_eq!(status, 0);
    }

    #[tokio::test]
    async fn subscribing_then_dropping_the_receiver_does_not_panic() {
        let backend = CoreAudioBackend::default();
        let (tx, rx) = watch::channel(0u64);
        backend.subscribe(tx);
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        drop(rx);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        // A write still bumps the (now receiver-less) channel without panicking.
        let _ = backend
            .set_mute(&Node::Sink("no-such-uid".into()), Mute::On)
            .await;
    }

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
