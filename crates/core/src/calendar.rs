use std::process::Command;
use std::time::Duration;

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
const SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(10);

/// Run a Command with a timeout. Returns None if the process hangs or fails to start.
///
/// On Unix, the child is placed in its own process group so that the entire
/// group (including any grandchild processes) can be killed on timeout.
pub(crate) fn output_with_timeout(
    mut cmd: Command,
    timeout: Duration,
) -> Option<std::process::Output> {
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
                    // Kill the process group (negative PID) to catch any children
                    libc::kill(-(child_id as i32), libc::SIGKILL);
                }
            }
            drop(handle); // detach — thread exits on its own after kill
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
        // Try compiled EventKit helper first
        if let Some(events) = query_via_eventkit(lookahead_minutes) {
            if !events.is_empty() {
                return events;
            }
        }
        // AppleScript: fetch today's events, filter by time range
        query_via_applescript(lookahead_minutes)
    }
}

/// Find calendar events that overlap a given time window.
/// Used to match a recording to its calendar event after the fact.
/// On non-macOS platforms, returns an empty list.
pub fn events_overlapping_now() -> Vec<CalendarEvent> {
    #[cfg(not(target_os = "macos"))]
    {
        return Vec::new();
    }
    #[cfg(target_os = "macos")]
    // Query events in a 2-hour window centered on now (covers most meetings)
    // The AppleScript returns events starting within the window;
    // we also look backward to catch events that started before recording began.
    query_events_with_attendees()
}

/// AppleScript query that fetches current/recent events WITH attendee names.
fn query_events_with_attendees() -> Vec<CalendarEvent> {
    let script = r#"set now to current date
set windowStart to now - (2 * 60 * 60)
set windowEnd to now + (2 * 60 * 60)
set todayStart to current date
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
return output"#;

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

    events.sort_by_key(|e| (e.minutes_until.abs(), e.title.clone()));
    events.dedup_by(|a, b| a.title == b.title);
    events
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
    let events: Vec<CalendarEvent> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();

    Some(events)
}

/// Find the compiled calendar-events helper binary.
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
    let dev = dirs::home_dir()
        .unwrap_or_default()
        .join("Sites/minutes/target/release/calendar-events");
    if dev.exists() {
        return Some(dev);
    }
    None
}

/// AppleScript approach: fetch ALL events for today+tomorrow, filter by time.
/// Avoids `whose start date >= ...` which times out on CalDAV calendars.
fn query_via_applescript(lookahead_minutes: u32) -> Vec<CalendarEvent> {
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
}
