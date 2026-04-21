use std::process::Command;
use std::time::Duration;

use chrono::{DateTime, Datelike, Local, Timelike};

// ──────────────────────────────────────────────────────────────
// Calendar integration — upcoming meetings from macOS Calendar.
//
// Uses AppleScript to query Calendar.app. Avoids the `whose`
// filter on CalDAV calendars (causes timeouts). Instead fetches
// all events for today and filters by time in the script.
//
// Also tries a compiled EventKit helper if available.
// ──────────────────────────────────────────────────────────────

/// Maximum time to wait for a calendar subprocess before giving up.
/// Calendar queries should complete in <1s when Calendar.app is healthy.
/// 3s is generous enough for slow CalDAV syncs but doesn't freeze the app.
const SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(3);
const EVENTKIT_OVERLAP_LOOKAHEAD_MINUTES: u32 = 120;
const EVENTKIT_OVERLAP_LOOKBACK_MINUTES: u32 = 120;

/// Run a Command with a timeout. Returns None if the process hangs or fails to start.
///
/// On Unix, the child is placed in its own process group so that the entire
/// group (including any grandchild processes) can be killed on timeout.
pub(crate) fn output_with_timeout(
    mut cmd: Command,
    timeout: Duration,
) -> Option<std::process::Output> {
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    // Put the child in its own process group so we can kill the whole group.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setpgid is async-signal-safe and called between fork/exec.
        unsafe {
            cmd.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }
    }

    let child = cmd.spawn().ok()?;

    let (tx, rx) = std::sync::mpsc::channel();
    let child_id = child.id();
    let handle = std::thread::spawn(move || {
        let result = child.wait_with_output();
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => {
            let _ = handle.join();
            result.ok()
        }
        Err(_) => {
            // Timed out — kill the entire process group
            eprintln!(
                "[calendar] subprocess {} timed out after {:?}, killing",
                child_id, timeout
            );
            #[cfg(unix)]
            {
                unsafe {
                    libc::kill(-(child_id as i32), libc::SIGKILL);
                    // Also kill the PID directly in case setpgid raced
                    libc::kill(child_id as i32, libc::SIGKILL);
                }
            }
            // Wait briefly for the waiter thread to notice the kill and exit,
            // so we don't leak threads + zombie processes across retries.
            let _ = handle.join().ok();
            None
        }
    }
}

/// A calendar event with title, start time, attendees, and optional meeting URL.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CalendarEvent {
    pub title: String,
    pub start: String,
    pub minutes_until: i64,
    #[serde(default)]
    pub attendees: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
}

/// Extract a meeting URL (Zoom, Google Meet, Teams, Webex) from text.
/// Searches for common video conferencing URL patterns and returns the first match.
pub fn extract_meeting_url(text: &str) -> Option<String> {
    let patterns = [
        "https://zoom.us/j/",
        "https://us02web.zoom.us/j/",
        "https://us04web.zoom.us/j/",
        "https://us05web.zoom.us/j/",
        "https://us06web.zoom.us/j/",
        "https://meet.google.com/",
        "https://teams.microsoft.com/l/meetup-join/",
        "https://teams.live.com/meet/",
        "https://webex.com/meet/",
        "https://facetime.apple.com/",
    ];

    for pattern in &patterns {
        if let Some(start) = text.find(pattern) {
            let url_text = &text[start..];
            let end = url_text
                .find(|c: char| c.is_whitespace() || c == '>' || c == '"' || c == ')')
                .unwrap_or(url_text.len());
            let url = &url_text[..end];
            if url.len() > pattern.len() {
                return Some(url.to_string());
            }
        }
    }

    // Fallback: look for any https:// URL containing common meeting keywords
    for keyword in &[
        "zoom.us",
        "meet.google",
        "teams.microsoft",
        "webex.com",
        "facetime.apple",
    ] {
        if let Some(https_pos) = text.find("https://") {
            let url_text = &text[https_pos..];
            if url_text.contains(keyword) {
                let end = url_text
                    .find(|c: char| c.is_whitespace() || c == '>' || c == '"' || c == ')')
                    .unwrap_or(url_text.len());
                return Some(url_text[..end].to_string());
            }
        }
    }

    None
}

/// Query upcoming calendar events within the next `lookahead_minutes`.
/// Returns events sorted by start time (all-day events excluded).
/// On non-macOS platforms, returns an empty list (calendar integration uses AppleScript/EventKit).
pub fn upcoming_events(lookahead_minutes: u32) -> Vec<CalendarEvent> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = lookahead_minutes;
        return Vec::new();
    }
    #[cfg(target_os = "macos")]
    {
        if !calendar_integration_enabled() {
            return Vec::new();
        }
        // Try compiled EventKit helper first (fastest path, no Apple Events)
        if let Some(events) = query_via_eventkit(lookahead_minutes) {
            // EventKit helper responded — use its result even if empty
            // (empty means no events, not a failure). Don't fall through
            // to AppleScript, which would double the timeout if Calendar
            // is hung since both paths talk to the same backend.
            return events;
        }
        // AppleScript fallback: only reached when EventKit helper is missing
        query_via_applescript(lookahead_minutes)
    }
}

/// Find calendar events that overlap a given time window.
/// Used to match a recording to its calendar event after the fact.
/// On non-macOS platforms, returns an empty list.
pub fn events_overlapping(at: DateTime<Local>) -> Vec<CalendarEvent> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = at;
        Vec::new()
    }
    #[cfg(target_os = "macos")]
    {
        if !calendar_integration_enabled() {
            return Vec::new();
        }

        // Preserve the fast helper path for "right now" lookups, but allow
        // historical reprocessing to center the query on the recording time.
        if (Local::now() - at).num_seconds().abs() <= 60 {
            return events_overlapping_now();
        }

        if let Some(events) = query_overlap_via_eventkit(Some(at.timestamp())) {
            return events;
        }

        query_events_with_attendees_at(at)
    }
}

pub fn events_overlapping_now() -> Vec<CalendarEvent> {
    #[cfg(not(target_os = "macos"))]
    {
        return Vec::new();
    }
    #[cfg(target_os = "macos")]
    {
        if !calendar_integration_enabled() {
            return Vec::new();
        }
        // Try EventKit helper first (sub-second, no CalDAV round-trips).
        // Pass lookahead=120, lookback=120 for a 4-hour window centered on now.
        if let Some(events) = query_overlap_via_eventkit(None) {
            return events;
        }
        // AppleScript fallback: only reached when EventKit helper is missing
        query_events_with_attendees()
    }
}

/// Returns `false` when the user has set `[calendar] enabled = false`.
/// Every caller in this module consults this before touching Calendar so
/// an opted-out user never sees AppleScript launch Calendar.app.
#[cfg(target_os = "macos")]
fn calendar_integration_enabled() -> bool {
    crate::config::Config::load().calendar.enabled
}

/// `true` when Calendar.app is currently running.
///
/// Used as a last-line guard before any `tell application "Calendar"`
/// AppleScript. `tell application` auto-launches the target app, which
/// meant every 60s poll launched Calendar.app for users who never use it.
/// `pgrep -x Calendar` is ~1ms and does not trigger any TCC prompts.
#[cfg(target_os = "macos")]
fn is_calendar_app_running() -> bool {
    Command::new("pgrep")
        .args(["-x", "Calendar"])
        .output()
        .map(|out| out.status.success() && !out.stdout.is_empty())
        .unwrap_or(false)
}

/// AppleScript query that fetches current/recent events WITH attendee names.
fn query_events_with_attendees() -> Vec<CalendarEvent> {
    query_events_with_attendees_at(Local::now())
}

fn applescript_month(month: u32) -> &'static str {
    match month {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => "January",
    }
}

/// AppleScript query centered on an explicit timestamp.
fn query_events_with_attendees_at(center: DateTime<Local>) -> Vec<CalendarEvent> {
    // Never auto-launch Calendar.app from a background query. `tell
    // application "Calendar"` launches the app as a side effect, which
    // surprised users who don't use Apple Calendar.
    if !is_calendar_app_running() {
        return Vec::new();
    }
    let script = r#"set now to current date
set year of now to __YEAR__
set month of now to __MONTH__
set day of now to __DAY__
set hours of now to __HOUR__
set minutes of now to __MINUTE__
set seconds of now to __SECOND__
set windowStart to now - (2 * 60 * 60)
set windowEnd to now + (2 * 60 * 60)
set todayStart to current date
set year of todayStart to __YEAR__
set month of todayStart to __MONTH__
set day of todayStart to __DAY__
set hours of todayStart to 0
set minutes of todayStart to 0
set seconds of todayStart to 0
set tomorrowEnd to todayStart + (2 * 24 * 60 * 60)
set output to ""
set unitSep to (ASCII character 31)
set fieldSep to (ASCII character 30)
tell application "Calendar"
    repeat with cal in calendars
        try
            set evts to (every event of cal whose start date >= todayStart and start date <= tomorrowEnd)
            repeat with evt in evts
                set s to start date of evt
                set e to end date of evt
                if (s <= windowEnd and e >= windowStart) then
                    set t to summary of evt
                    set mins to ((s - now) / 60) as integer
                    set attendeeNames to ""
                    try
                        set theAttendees to attendees of evt
                        repeat with anAttendee in theAttendees
                            if attendeeNames is not "" then
                                set attendeeNames to attendeeNames & fieldSep
                            end if
                            set attendeeNames to attendeeNames & (name of anAttendee)
                        end repeat
                    end try
                    set loc to ""
                    try
                        set loc to location of evt
                        if loc is missing value then set loc to ""
                    end try
                    set output to output & t & unitSep & (s as string) & unitSep & mins & unitSep & attendeeNames & unitSep & loc & linefeed
                end if
            end repeat
        end try
    end repeat
end tell
return output"#
        .replace("__YEAR__", &center.year().to_string())
        .replace("__MONTH__", applescript_month(center.month()))
        .replace("__DAY__", &center.day().to_string())
        .replace("__HOUR__", &center.hour().to_string())
        .replace("__MINUTE__", &center.minute().to_string())
        .replace("__SECOND__", &center.second().to_string());

    let mut cmd = Command::new("osascript");
    cmd.arg("-e").arg(script);
    let output = match output_with_timeout(cmd, SUBPROCESS_TIMEOUT) {
        Some(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return Vec::new(),
    };

    let unit_sep = '\x1F';
    let field_sep = '\x1E';
    let mut events: Vec<CalendarEvent> = output
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(5, unit_sep).collect();
            if parts.len() >= 3 {
                let attendees = if parts.len() >= 4 && !parts[3].trim().is_empty() {
                    parts[3]
                        .split(field_sep)
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                } else {
                    Vec::new()
                };
                let url = parts.get(4).and_then(|loc| extract_meeting_url(loc.trim()));
                Some(CalendarEvent {
                    title: parts[0].trim().to_string(),
                    start: parts[1].trim().to_string(),
                    minutes_until: parts[2].trim().parse().unwrap_or(0),
                    attendees,
                    url,
                })
            } else {
                None
            }
        })
        .collect();

    dedup_events(&mut events);
    events
}

/// Parse EventKit helper JSON output, extracting meeting URLs from raw location strings.
fn parse_eventkit_output(stdout: &str) -> Vec<CalendarEvent> {
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            let mut event: CalendarEvent = serde_json::from_str(line).ok()?;
            // The helper outputs the raw location string as url —
            // extract an actual meeting URL (Zoom, Meet, Teams, etc.) or set to None.
            event.url = event.url.as_deref().and_then(extract_meeting_url);
            Some(event)
        })
        .collect()
}

/// Deduplicate events by title, keeping the one closest to now.
fn dedup_events(events: &mut Vec<CalendarEvent>) {
    events.sort_by_key(|e| (e.title.clone(), e.minutes_until.abs()));
    events.dedup_by(|a, b| a.title == b.title);
    // Re-sort by proximity to now (most relevant first)
    events.sort_by_key(|e| e.minutes_until.abs());
}

/// Query via compiled Swift EventKit helper (if available and permitted).
fn query_via_eventkit(lookahead_minutes: u32) -> Option<Vec<CalendarEvent>> {
    let helper = find_calendar_helper()?;

    let mut cmd = Command::new(&helper);
    cmd.arg(lookahead_minutes.to_string());
    let output = output_with_timeout(cmd, SUBPROCESS_TIMEOUT)?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Some(parse_eventkit_output(&stdout))
}

/// Query overlapping events via EventKit helper with a backward+forward window.
/// Returns None if the helper is missing or fails (triggers AppleScript fallback).
fn query_overlap_via_eventkit(reference_epoch_seconds: Option<i64>) -> Option<Vec<CalendarEvent>> {
    let helper = find_calendar_helper()?;
    query_overlap_via_eventkit_with_helper(&helper, reference_epoch_seconds)
}

fn query_overlap_via_eventkit_with_helper(
    helper: &std::path::Path,
    reference_epoch_seconds: Option<i64>,
) -> Option<Vec<CalendarEvent>> {
    tracing::info!(
        lookahead_minutes = EVENTKIT_OVERLAP_LOOKAHEAD_MINUTES,
        lookback_minutes = EVENTKIT_OVERLAP_LOOKBACK_MINUTES,
        reference_epoch_seconds,
        "querying calendar overlap via EventKit helper"
    );

    let mut cmd = Command::new(helper);
    cmd.arg(EVENTKIT_OVERLAP_LOOKAHEAD_MINUTES.to_string())
        .arg(EVENTKIT_OVERLAP_LOOKBACK_MINUTES.to_string());
    if let Some(reference_epoch_seconds) = reference_epoch_seconds {
        cmd.arg(reference_epoch_seconds.to_string());
    }
    let output = output_with_timeout(cmd, SUBPROCESS_TIMEOUT)?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut events = parse_eventkit_output(&stdout);
    dedup_events(&mut events);
    Some(events)
}

/// Find the compiled calendar-events helper binary.
///
/// Lookup order:
/// 1. `<exe>/../Resources/calendar-events` — inside a packaged .app bundle
///    (populated by Tauri's bundler from `tauri/src-tauri/resources/calendar-events`)
/// 2. `<exe>/calendar-events` — beside the main binary
/// 3. Workspace `tauri/src-tauri/resources/calendar-events` — dev fallback.
///    `tauri/src-tauri/build.rs` compiles the Swift helper here on every
///    `cargo tauri build` / `cargo tauri dev`, so the CLI finds it when
///    running from source after at least one Tauri build has completed.
/// 4. Workspace `target/release/calendar-events` — legacy dev fallback for
///    older local workflows that compiled the helper directly into `target/`.
///
/// The workspace root is derived from `CARGO_MANIFEST_DIR` at compile time
/// so it works regardless of where the user cloned the repo.
fn find_calendar_helper() -> Option<std::path::PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let in_resources = dir.join("../Resources/calendar-events");
            if in_resources.exists() {
                return Some(in_resources);
            }
            let beside = dir.join("calendar-events");
            if beside.exists() {
                return Some(beside);
            }
        }
    }
    // CARGO_MANIFEST_DIR points at crates/core; the workspace root is two up.
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent());
    if let Some(root) = workspace_root {
        let staged = root.join("tauri/src-tauri/resources/calendar-events");
        if staged.exists() {
            return Some(staged);
        }
        let legacy = root.join("target/release/calendar-events");
        if legacy.exists() {
            return Some(legacy);
        }
    }
    None
}

/// AppleScript approach: fetch ALL events for today+tomorrow, filter by time.
/// Avoids `whose start date >= ...` which times out on CalDAV calendars.
fn query_via_applescript(lookahead_minutes: u32) -> Vec<CalendarEvent> {
    // See `query_events_with_attendees_at`: skip when Calendar.app isn't
    // already running so periodic polling never auto-launches the app.
    if !is_calendar_app_running() {
        return Vec::new();
    }
    // Fetch events for a 2-day window, then filter in the script
    let script = format!(
        r#"set now to current date
set todayStart to current date
set hours of todayStart to 0
set minutes of todayStart to 0
set seconds of todayStart to 0
set tomorrowEnd to todayStart + (2 * 24 * 60 * 60)
set lookaheadSecs to {minutes} * 60
set horizon to now + lookaheadSecs
set output to ""
tell application "Calendar"
    repeat with cal in calendars
        try
            set evts to (every event of cal whose start date >= todayStart and start date <= tomorrowEnd)
            repeat with evt in evts
                set s to start date of evt
                if s >= now and s <= horizon then
                    set t to summary of evt
                    set mins to ((s - now) / 60) as integer
                    set loc to ""
                    try
                        set loc to location of evt
                        if loc is missing value then set loc to ""
                    end try
                    set output to output & t & (ASCII character 31) & (s as string) & (ASCII character 31) & mins & (ASCII character 31) & loc & linefeed
                end if
            end repeat
        end try
    end repeat
end tell
return output"#,
        minutes = lookahead_minutes
    );

    let mut cmd = Command::new("osascript");
    cmd.arg("-e").arg(&script);
    let output = match output_with_timeout(cmd, SUBPROCESS_TIMEOUT) {
        Some(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        Some(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            eprintln!("[calendar] applescript failed: {}", stderr.trim());
            return Vec::new();
        }
        None => {
            eprintln!("[calendar] osascript timed out or failed to start");
            return Vec::new();
        }
    };

    let sep = '\x1F'; // ASCII unit separator
    let mut events: Vec<CalendarEvent> = output
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(4, sep).collect();
            if parts.len() >= 3 {
                let url = parts.get(3).and_then(|loc| extract_meeting_url(loc.trim()));
                Some(CalendarEvent {
                    title: parts[0].trim().to_string(),
                    start: parts[1].trim().to_string(),
                    minutes_until: parts[2].trim().parse().unwrap_or(0),
                    attendees: Vec::new(),
                    url,
                })
            } else {
                None
            }
        })
        .collect();

    // Deduplicate by title (same event can appear in multiple calendars)
    events.sort_by_key(|e| (e.minutes_until, e.title.clone()));
    events.dedup_by(|a, b| a.title == b.title && a.minutes_until == b.minutes_until);
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn extract_zoom_url() {
        let text = "https://zoom.us/j/1234567890?pwd=abc123";
        assert_eq!(
            extract_meeting_url(text),
            Some("https://zoom.us/j/1234567890?pwd=abc123".to_string())
        );
    }

    #[test]
    fn extract_google_meet_url() {
        let text = "Join: https://meet.google.com/abc-defg-hij";
        assert_eq!(
            extract_meeting_url(text),
            Some("https://meet.google.com/abc-defg-hij".to_string())
        );
    }

    #[test]
    fn extract_teams_url() {
        let text = "https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc";
        assert_eq!(
            extract_meeting_url(text),
            Some("https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc".to_string())
        );
    }

    #[test]
    fn extract_no_url() {
        assert_eq!(extract_meeting_url("Conference Room B"), None);
        assert_eq!(extract_meeting_url(""), None);
        assert_eq!(extract_meeting_url("https://docs.google.com/doc/123"), None);
    }

    #[test]
    fn extract_url_from_mixed_text() {
        let text = "Location: Building 4, Room 201\nhttps://zoom.us/j/999 (backup link)";
        assert_eq!(
            extract_meeting_url(text),
            Some("https://zoom.us/j/999".to_string())
        );
    }

    #[test]
    fn extract_zoom_subdomain_url() {
        let text = "https://us02web.zoom.us/j/8765432?pwd=xyz";
        assert_eq!(
            extract_meeting_url(text),
            Some("https://us02web.zoom.us/j/8765432?pwd=xyz".to_string())
        );
    }

    // Tests that shell out to a POSIX helper script are unix-only. On
    // Windows there is no chmod +x equivalent and the fake helper below
    // wouldn't execute as a script. The behavior being tested
    // (argument-passing contract between Minutes and the EventKit
    // helper) is itself macOS-only anyway, so there's nothing to validate
    // on Windows.
    #[cfg(unix)]
    #[test]
    fn query_overlap_via_eventkit_passes_reference_timestamp() {
        let tempdir = tempfile::tempdir().unwrap();
        let helper_path = tempdir.path().join("calendar-events");
        let args_path = tempdir.path().join("args.txt");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
            args_path.display()
        );
        std::fs::write(&helper_path, script).unwrap();
        let mut permissions = std::fs::metadata(&helper_path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&helper_path, permissions).unwrap();

        let events = query_overlap_via_eventkit_with_helper(&helper_path, Some(1_700_000_000))
            .expect("helper should run");
        assert!(events.is_empty());

        let args = std::fs::read_to_string(&args_path).unwrap();
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            ["120", "120", "1700000000"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn query_overlap_via_eventkit_omits_reference_timestamp_when_not_provided() {
        let tempdir = tempfile::tempdir().unwrap();
        let helper_path = tempdir.path().join("calendar-events");
        let args_path = tempdir.path().join("args.txt");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
            args_path.display()
        );
        std::fs::write(&helper_path, script).unwrap();
        let mut permissions = std::fs::metadata(&helper_path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&helper_path, permissions).unwrap();

        let events =
            query_overlap_via_eventkit_with_helper(&helper_path, None).expect("helper should run");
        assert!(events.is_empty());

        let args = std::fs::read_to_string(&args_path).unwrap();
        assert_eq!(args.lines().collect::<Vec<_>>(), ["120", "120"]);
    }
}
