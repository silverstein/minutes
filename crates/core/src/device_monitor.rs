use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

// ──────────────────────────────────────────────────────────────
// Device change monitor.
//
// Detects when the system default audio input device changes
// (e.g., Bluetooth headset connects/disconnects mid-recording).
//
// On macOS: registers a CoreAudio property listener for instant
// notification via AudioObjectAddPropertyListener. Falls back to
// polling on other platforms.
//
// Used by capture.rs and streaming consumers (live_transcript,
// dictation) to trigger automatic stream reconnection.
// ──────────────────────────────────────────────────────────────

/// Minimum time between reconnection attempts (debounce).
const RECONNECT_DEBOUNCE_SECS: u64 = 2;

/// Monitors the system default audio input device for changes.
pub struct DeviceMonitor {
    /// The device name we're currently recording from.
    current_device: String,
    /// Set to true when the default input device changes.
    device_changed: Arc<AtomicBool>,
    /// Debounce: when the last reconnection happened.
    last_reconnect: Instant,
    /// macOS CoreAudio listener handle (unregisters on drop).
    #[cfg(target_os = "macos")]
    _listener: Option<coreaudio_listener::CoreAudioListener>,
}

impl DeviceMonitor {
    /// Create a new monitor tracking the given device name.
    /// On macOS, registers a CoreAudio listener for instant device-change notification.
    pub fn new(initial_device: &str) -> Self {
        let device_changed = Arc::new(AtomicBool::new(false));

        #[cfg(target_os = "macos")]
        let _listener = coreaudio_listener::CoreAudioListener::new(Arc::clone(&device_changed));

        Self {
            current_device: initial_device.to_string(),
            device_changed,
            last_reconnect: Instant::now(),
            #[cfg(target_os = "macos")]
            _listener,
        }
    }

    /// Check if the default input device has changed from the one we're using.
    ///
    /// On macOS: O(1) atomic check (CoreAudio callback sets the flag).
    /// On other platforms: queries the current default and compares names.
    /// Respects the debounce interval to prevent rapid reconnection thrashing.
    pub fn has_device_changed(&self) -> bool {
        // Debounce: don't trigger again within RECONNECT_DEBOUNCE_SECS of last reconnect
        if self.last_reconnect.elapsed().as_secs() < RECONNECT_DEBOUNCE_SECS {
            return false;
        }

        #[cfg(target_os = "macos")]
        {
            // Fast path: CoreAudio listener sets this flag immediately
            if self.device_changed.load(Ordering::Relaxed) {
                return true;
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            // Poll fallback: check if cpal's default device name differs from ours
            if let Some(current_default) = self.current_default_name() {
                if current_default != self.current_device {
                    return true;
                }
            }
        }

        false
    }

    /// Get the current system default input device name.
    pub fn current_default_name(&self) -> Option<String> {
        #[cfg(target_os = "macos")]
        {
            crate::capture::get_macos_default_input_name()
        }
        #[cfg(not(target_os = "macos"))]
        {
            use cpal::traits::{DeviceTrait, HostTrait};
            cpal::default_host()
                .default_input_device()
                .and_then(|d| d.name().ok())
        }
    }

    /// Update tracked state after a successful reconnection.
    pub fn update_device(&mut self, new_device: &str) {
        self.current_device = new_device.to_string();
        self.device_changed.store(false, Ordering::Relaxed);
        self.last_reconnect = Instant::now();
    }

    /// The device name currently being tracked.
    pub fn current_device_name(&self) -> &str {
        &self.current_device
    }
}

// ── macOS CoreAudio property listener ────────────────────────

#[cfg(target_os = "macos")]
mod coreaudio_listener {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    // CoreAudio constants (from AudioHardware.h)
    const K_AUDIO_OBJECT_SYSTEM_OBJECT: u32 = 1;
    // 'dIn ' — kAudioHardwarePropertyDefaultInputDevice
    const K_AUDIO_HARDWARE_PROPERTY_DEFAULT_INPUT_DEVICE: u32 = 0x64496E20;
    // 'glob' — kAudioObjectPropertyScopeGlobal
    const K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL: u32 = 0x676C6F62;
    // kAudioObjectPropertyElementMain
    const K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN: u32 = 0;

    #[repr(C)]
    struct AudioObjectPropertyAddress {
        selector: u32,
        scope: u32,
        element: u32,
    }

    type AudioObjectPropertyListenerProc = extern "C" fn(
        u32,                               // inObjectID
        u32,                               // inNumberAddresses
        *const AudioObjectPropertyAddress, // inAddresses
        *mut std::ffi::c_void,             // inClientData
    ) -> i32;

    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        fn AudioObjectAddPropertyListener(
            object_id: u32,
            address: *const AudioObjectPropertyAddress,
            listener: AudioObjectPropertyListenerProc,
            client_data: *mut std::ffi::c_void,
        ) -> i32;

        fn AudioObjectRemovePropertyListener(
            object_id: u32,
            address: *const AudioObjectPropertyAddress,
            listener: AudioObjectPropertyListenerProc,
            client_data: *mut std::ffi::c_void,
        ) -> i32;
    }

    extern "C" fn device_change_callback(
        _object_id: u32,
        _num_addresses: u32,
        _addresses: *const AudioObjectPropertyAddress,
        client_data: *mut std::ffi::c_void,
    ) -> i32 {
        // Safety: client_data is a raw pointer to our Arc's inner AtomicBool,
        // kept alive by the CoreAudioListener struct.
        let flag = unsafe { &*(client_data as *const AtomicBool) };
        flag.store(true, Ordering::Relaxed);
        tracing::debug!("CoreAudio: default input device changed");
        0 // noErr
    }

    fn default_input_address() -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress {
            selector: K_AUDIO_HARDWARE_PROPERTY_DEFAULT_INPUT_DEVICE,
            scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
            element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
        }
    }

    /// Handle to a registered CoreAudio property listener.
    /// Unregisters the listener on drop.
    pub struct CoreAudioListener {
        /// Prevent the Arc from being dropped while CoreAudio holds the raw pointer.
        _flag: Arc<AtomicBool>,
        /// Raw pointer passed to CoreAudio as client_data.
        client_data: *mut std::ffi::c_void,
    }

    // Safety: the AtomicBool is Send+Sync, and we only read/write it atomically.
    unsafe impl Send for CoreAudioListener {}
    unsafe impl Sync for CoreAudioListener {}

    impl CoreAudioListener {
        /// Register a CoreAudio listener for default input device changes.
        /// Returns `None` if registration fails (non-fatal — caller falls back to polling).
        pub fn new(flag: Arc<AtomicBool>) -> Option<Self> {
            let address = default_input_address();
            // Get a raw pointer to the AtomicBool inside the Arc.
            // The Arc is kept alive by the CoreAudioListener struct.
            let client_data = Arc::as_ptr(&flag) as *mut std::ffi::c_void;

            let status = unsafe {
                AudioObjectAddPropertyListener(
                    K_AUDIO_OBJECT_SYSTEM_OBJECT,
                    &address,
                    device_change_callback,
                    client_data,
                )
            };

            if status != 0 {
                tracing::warn!(
                    status,
                    "failed to register CoreAudio default input listener"
                );
                return None;
            }

            tracing::debug!("CoreAudio default input device listener registered");
            Some(CoreAudioListener {
                _flag: flag,
                client_data,
            })
        }
    }

    impl Drop for CoreAudioListener {
        fn drop(&mut self) {
            let address = default_input_address();
            let status = unsafe {
                AudioObjectRemovePropertyListener(
                    K_AUDIO_OBJECT_SYSTEM_OBJECT,
                    &address,
                    device_change_callback,
                    self.client_data,
                )
            };
            if status != 0 {
                tracing::warn!(status, "failed to unregister CoreAudio listener");
            } else {
                tracing::debug!("CoreAudio default input device listener unregistered");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_monitor_tracks_name() {
        let mon = DeviceMonitor::new("Test Device");
        assert_eq!(mon.current_device_name(), "Test Device");
    }

    #[test]
    fn update_device_resets_state() {
        let mut mon = DeviceMonitor::new("Old Device");
        mon.device_changed.store(true, Ordering::Relaxed);
        mon.update_device("New Device");
        assert_eq!(mon.current_device_name(), "New Device");
        assert!(!mon.device_changed.load(Ordering::Relaxed));
    }

    #[test]
    fn debounce_prevents_rapid_changes() {
        let mut mon = DeviceMonitor::new("Device A");
        // Simulate a just-happened reconnect
        mon.update_device("Device A");
        // Even if flag is set, debounce should prevent triggering
        mon.device_changed.store(true, Ordering::Relaxed);
        assert!(!mon.has_device_changed());
    }
}
