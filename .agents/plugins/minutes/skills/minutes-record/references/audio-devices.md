# Audio routes for Minutes

Use the recording preflight for the requested intent. An input device alone does not prove that both sides of a call will be captured.

| Scenario | Supported starting point |
| --- | --- |
| In-person conversation | Selected built-in or external microphone |
| Mac call | Running Minutes desktop app with its native system-audio route and required permissions |
| Deliberately configured virtual-device route | BlackHole or another supported configured input, checked by call preflight |
| Imported voice memo | Existing local file or configured watcher; microphone capture is unnecessary |

## Native Mac call capture

Use the connected `start_recording` tool with call intent. It runs call-aware preflight and delegates to the desktop app when required. Check the returned route, accepted state and recording status. The installed AI plugin cannot grant microphone or system-audio permissions.

If the desktop app is unavailable, explain that requirement. If system audio is unavailable, report the actual preflight failure. Do not silently downgrade to microphone-only capture: this can save only the user's voice and lose the other participants.

Bluetooth/headset routing can differ by hardware and capture backend. Check the actual audio probe and saved test before treating a visible device name as a working route. Do not change the user's output device or capture backend without a requested setup change.

## Optional BlackHole route

BlackHole is useful for an explicitly configured virtual-input workflow; it is not required by every supported Mac recording.

```bash
brew install blackhole-2ch
```

In Audio MIDI Setup, create a Multi-Output Device containing the user's speakers/headphones and BlackHole, then select that output for the call. Configure the matching Minutes input deliberately. A BlackHole input by itself does not include the user's microphone; verify both sides of the intended recording rather than assuming they are mixed.

If the user temporarily changes system output, help restore their previous output after the call. Preserve any existing aggregate-device setup. Never mute the user's microphone as a generic recording fix.

## Troubleshooting

| Observation | Next check |
| --- | --- |
| Empty or quiet capture | Microphone permission for the responsible process, selected input and the actual audio probe |
| Only the user's voice in a call | System-audio route and call preflight; microphone permission alone is insufficient |
| Connection works but recording fails | Native app availability, capture ownership, speech-model readiness and the specific runtime permission error |
| Dictation captures but cannot insert | Destination and Accessibility readiness; separate from microphone access |
| Notification sounds appear in a transcript | System audio can contain those sounds; use the user's preferred notification settings when requested |

For setup help: https://github.com/silverstein/minutes/discussions.
