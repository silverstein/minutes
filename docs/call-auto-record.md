# Automatic call recording

On macOS, Minutes can automatically record selected Teams and Zoom desktop calls. This is off by default. In Settings under **AI & Privacy**, enable call detection, choose the desktop apps you want recorded, then enable **Record detected calls automatically**.

Automatic recordings use native capture for your microphone and this Mac's audio. The recording bar identifies the app and the automatic start. You can stop at any time. If native capture cannot start, Minutes reports the failure and leaves the manual call banner available. It does not substitute a microphone-only recording.

The equivalent configuration is:

```toml
[call_detection]
enabled = true
auto_record = true
auto_record_apps = ["MSTeams", "zoom.us"]
stop_when_call_ends = true
call_end_stop_countdown_secs = 30
```

`Microsoft Teams` and `Microsoft Teams (work or school)` are accepted aliases for `MSTeams`. An empty `auto_record_apps` list starts nothing. Browser calls and other desktop apps remain manual, even if listed here. The app must also be enabled in the existing detection `apps` list.

Detection requires microphone activity attributed to the call app's process family. Each detected call gets one automatic start attempt. Reminders, a failed attempt, and stopping while the same call remains detected do not trigger another attempt. Thirty seconds without a detected call rearms automatic recording; a long mute can therefore look like a call ending. Minutes cannot distinguish every app's mute, hold and hang-up states.

The existing **Stop when call ends** option applies to automatic recordings as well as banner-started recordings. Its countdown still offers **Keep recording**. Auto-pause is not included.

Recording, dictation and Live keep their existing capture-ownership checks. If another capture owns the microphone, automatic recording does not interrupt it. A configuration that requires consent confirmation continues to require a manual start. Automatic start requests, failures and successful native starts include the app and detection source in the activity log.
