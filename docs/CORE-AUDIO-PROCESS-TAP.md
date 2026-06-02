# Core Audio Process Tap Backend

Minutes can opt into Apple's Core Audio Process Tap API for macOS system-audio
capture with:

```toml
[recording]
capture_backend = "core-audio-tap"

[recording.sources]
voice = "default"
call = "auto"
```

This backend is intentionally opt-in while the existing `cpal` loopback-device
path remains the default.

## Runtime Target

The code gates Process Tap startup to macOS 14.4 or newer. Apple's
`AudioHardwareCreateProcessTap` header is available as of macOS 14.2, but the
Rust binding and community examples target the 14.4 generation, so 14.4 is the
minimum supported runtime for this backend.

The app bundle deployment target stays lower for now. Older macOS releases can
still run Minutes and use the `cpal` backend; only the explicit
`core-audio-tap` backend returns unavailable below macOS 14.4.

## Permissions

The macOS bundle includes:

- `NSAudioCaptureUsageDescription` for Process Tap system-audio access
- `NSMicrophoneUsageDescription` for mic capture
- `NSScreenCaptureUsageDescription` for the older ScreenCaptureKit helper and
  optional screenshot context

There is no reliable permission-status shortcut in this PR. A Process Tap can
be created but still deliver silence, so the readiness probe remains the source
of truth: startup health must observe non-zero frames and non-silent RMS before
the system-audio route is treated as healthy.

## Entitlements and Notarization

No new hardened-runtime entitlement was added for Process Tap in this slice.
The existing Tauri entitlements already include microphone audio input and
user-selected file access. Release notarization should not need a new entitlement
for the tap backend, but signed builds must carry the updated Info.plist string
so macOS can present the correct privacy prompt.
