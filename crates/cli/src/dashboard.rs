//! Meeting Intelligence Dashboard — local HTTP server
//!
//! `minutes dashboard` starts a lightweight HTTP server that serves the
//! dashboard HTML and a JSON API endpoint (`GET /api/data`) built from
//! scanning the meetings directory.
//!
//! Architecture:
//!   TcpListener(:3141)
//!       ├── GET /          → embedded dashboard.html (include_str!)
//!       ├── GET /api/data  → JSON { meetings, stats, topics, warnings }
//!       └── *              → 404

use anyhow::{Context, Result};
use minutes_core::config::Config;
use minutes_core::markdown;
use minutes_core::pid;
use serde::Serialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;

static DASHBOARD_HTML: &str = include_str!("../assets/dashboard.html");

// ── Duration parsing ────────────────────────────────────────

/// Parse a free-form duration string to seconds.
///
/// Supports:
///   "45m"       → 2700
///   "1h 12m"    → 4320
///   "2h"        → 7200
///   "1:23:45"   → 5025
///   "23:45"     → 1425
///   ""          → 0
///   "garbage"   → 0
pub fn parse_duration(s: &str) -> u64 {
    let s = s.trim();
    if s.is_empty() {
        return 0;
    }

    // Try H:MM:SS or MM:SS format
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() == 3 {
        if let (Ok(h), Ok(m), Ok(sec)) = (
            parts[0].parse::<u64>(),
            parts[1].parse::<u64>(),
            parts[2].parse::<u64>(),
        ) {
            return h * 3600 + m * 60 + sec;
        }
    }
    if parts.len() == 2 {
        if let (Ok(m), Ok(sec)) = (parts[0].parse::<u64>(), parts[1].parse::<u64>()) {
            return m * 60 + sec;
        }
    }

    // Try Xh Ym format
    let mut total: u64 = 0;
    let mut found = false;
    let lower = s.to_lowercase();

    // Extract hours
    if let Some(h_pos) = lower.find('h') {
        if let Ok(h) = lower[..h_pos].trim().parse::<u64>() {
            total += h * 3600;
            found = true;
        }
    }

    // Extract minutes — look after 'h' if present, or from start
    let min_search = if let Some(h_pos) = lower.find('h') {
        &lower[h_pos + 1..]
    } else {
        &lower
    };
    if let Some(m_pos) = min_search.find('m') {
        if let Ok(m) = min_search[..m_pos].trim().parse::<u64>() {
            total += m * 60;
            found = true;
        }
    }

    // Extract seconds (Xs)
    if let Some(s_pos) = min_search.find('s') {
        let before_s = if let Some(m_pos) = min_search.find('m') {
            &min_search[m_pos + 1..s_pos]
        } else {
            &min_search[..s_pos]
        };
        if let Ok(sec) = before_s.trim().parse::<u64>() {
            total += sec;
            found = true;
        }
    }

    if found {
        total
    } else {
        0
    }
}

// ── Data structures ─────────────────────────────────────────

#[derive(Serialize)]
struct DashboardData {
    meetings: Vec<MeetingEntry>,
    stats: DashboardStats,
    topics: Vec<TopicEntry>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct MeetingEntry {
    title: String,
    date: String,
    duration: String,
    content_type: String,
    path: String,
    snippet: String,
    speaker_count: usize,
    decisions: Vec<serde_json::Value>,
    action_items: Vec<serde_json::Value>,
    source: String,
}

#[derive(Serialize)]
struct DashboardStats {
    total_meetings: usize,
    hours_captured: String,
    people_count: usize,
    duration_skipped: usize,
}

#[derive(Serialize)]
struct TopicEntry {
    tag: String,
    count: usize,
}

// ── Data collection ─────────────────────────────────────────

fn collect_dashboard_data(config: &Config) -> DashboardData {
    let dir = &config.output_dir;
    let mut meetings = Vec::new();
    let mut tag_counts: HashMap<String, usize> = HashMap::new();
    let mut total_seconds: u64 = 0;
    let mut duration_skipped: usize = 0;
    let mut people_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut warnings = Vec::new();

    if !dir.exists() {
        warnings.push(format!(
            "Meetings directory not found: {}. Using default ~/meetings/.",
            dir.display()
        ));
        return DashboardData {
            meetings: vec![],
            stats: DashboardStats {
                total_meetings: 0,
                hours_captured: "0".into(),
                people_count: 0,
                duration_skipped: 0,
            },
            topics: vec![],
            warnings,
        };
    }

    // Walk the meetings directory
    let walker = walkdir::WalkDir::new(dir)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().is_file()
                && e.path().extension().is_some_and(|ext| ext == "md")
                && !e
                    .path()
                    .components()
                    .any(|c| c.as_os_str() == "processed" || c.as_os_str() == "failed")
        });

    for entry in walker {
        let path = entry.path();
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let (fm_str, body) = markdown::split_frontmatter(&content);
        if fm_str.is_empty() {
            continue;
        }

        let title = markdown::extract_field(fm_str, "title").unwrap_or_default();
        let date = markdown::extract_field(fm_str, "date").unwrap_or_default();
        let duration = markdown::extract_field(fm_str, "duration").unwrap_or_default();
        let content_type =
            markdown::extract_field(fm_str, "type").unwrap_or_else(|| "meeting".into());
        let source = markdown::extract_field(fm_str, "source").unwrap_or_default();

        // Duration aggregation
        let secs = parse_duration(&duration);
        if secs > 0 {
            total_seconds += secs;
        } else if !duration.is_empty() {
            duration_skipped += 1;
        }

        // Parse full YAML for structured data
        let parsed: serde_yaml::Value = serde_yaml::from_str(fm_str).unwrap_or_default();

        // Tags → topic frequency
        if let Some(tags) = parsed.get("tags").and_then(|t| t.as_sequence()) {
            for tag in tags {
                if let Some(tag_str) = tag.as_str() {
                    *tag_counts.entry(tag_str.to_string()).or_insert(0) += 1;
                }
            }
        }

        // People tracking
        if let Some(attendees) = parsed.get("attendees").and_then(|a| a.as_sequence()) {
            for a in attendees {
                if let Some(name) = a.as_str() {
                    people_set.insert(name.to_string());
                }
            }
        }
        if let Some(people) = parsed.get("people").and_then(|p| p.as_sequence()) {
            for p in people {
                if let Some(name) = p.as_str() {
                    people_set.insert(name.to_string());
                }
            }
        }
        if let Some(sm) = parsed.get("speaker_map").and_then(|s| s.as_sequence()) {
            for entry in sm {
                if let Some(name) = entry.get("name").and_then(|n| n.as_str()) {
                    people_set.insert(name.to_string());
                }
            }
        }

        // Speaker count
        let speaker_count = parsed
            .get("speaker_map")
            .and_then(|s| s.as_sequence())
            .map_or(0, |s| s.len());

        // Decisions and action items
        let decisions: Vec<serde_json::Value> = parsed
            .get("decisions")
            .and_then(|d| d.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| serde_json::to_value(v).ok())
                    .collect()
            })
            .unwrap_or_default();

        let action_items: Vec<serde_json::Value> = parsed
            .get("action_items")
            .and_then(|a| a.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| serde_json::to_value(v).ok())
                    .collect()
            })
            .unwrap_or_default();

        // Snippet — first 200 chars of body
        let snippet: String = body
            .chars()
            .filter(|c| !c.is_control() || *c == ' ')
            .take(200)
            .collect::<String>()
            .trim()
            .to_string();

        meetings.push(MeetingEntry {
            title,
            date,
            duration,
            content_type,
            path: path.display().to_string(),
            snippet,
            speaker_count,
            decisions,
            action_items,
            source,
        });
    }

    // Sort by date descending
    meetings.sort_by(|a, b| b.date.cmp(&a.date));

    // Build topic list sorted by frequency
    let mut topics: Vec<TopicEntry> = tag_counts
        .into_iter()
        .map(|(tag, count)| TopicEntry { tag, count })
        .collect();
    topics.sort_by_key(|t| std::cmp::Reverse(t.count));

    // Format hours
    let hours = total_seconds as f64 / 3600.0;
    let hours_str = if hours < 1.0 {
        format!("{:.0}m", (total_seconds as f64 / 60.0))
    } else {
        format!("{:.1}", hours)
    };

    DashboardData {
        stats: DashboardStats {
            total_meetings: meetings.len(),
            hours_captured: hours_str,
            people_count: people_set.len(),
            duration_skipped,
        },
        meetings,
        topics,
        warnings,
    }
}

// ── Percent decoding ───────────────────────────────────────

fn percent_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        // Also decode '+' as space (form encoding)
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8(out).unwrap_or_default()
}

// ── HTTP server ─────────────────────────────────────────────

fn handle_request(stream: &mut std::net::TcpStream, config: &Config) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }

    // Parse method and path
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return;
    }
    let path = parts[1];

    // Consume remaining headers
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            break;
        }
    }

    // Parse query string for paths like /api/open?path=...
    let (route, query) = match path.split_once('?') {
        Some((r, q)) => (r, Some(q)),
        None => (path, None),
    };

    match route {
        "/" => {
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                DASHBOARD_HTML.len(),
                DASHBOARD_HTML
            );
            let _ = stream.write_all(response.as_bytes());
        }
        "/api/data" => {
            let data = collect_dashboard_data(config);
            let json = serde_json::to_string(&data).unwrap_or_else(|_| "{}".into());
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                json.len(),
                json
            );
            let _ = stream.write_all(response.as_bytes());
        }
        "/api/open" => {
            // Open a meeting file in the user's default app
            let file_path = query
                .and_then(|q| q.split('&').find_map(|param| param.strip_prefix("path=")))
                .map(percent_decode);

            let meetings_dir = config.output_dir.canonicalize().ok();
            let (status, body) = match (&meetings_dir, &file_path) {
                (Some(meetings_dir), Some(p)) => {
                    let candidate = std::path::Path::new(p);
                    let canon = candidate.canonicalize().unwrap_or_default();
                    if !canon.as_os_str().is_empty()
                        && canon.starts_with(meetings_dir)
                        && canon.extension().is_some_and(|e| e == "md")
                    {
                        let _ = std::process::Command::new("open").arg(p).spawn();
                        ("200 OK", "{\"ok\":true}")
                    } else {
                        ("403 Forbidden", "{\"error\":\"path outside meetings dir\"}")
                    }
                }
                _ => (
                    "400 Bad Request",
                    "{\"error\":\"missing path or meetings dir\"}",
                ),
            };
            let response = format!(
                "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                status, body.len(), body
            );
            let _ = stream.write_all(response.as_bytes());
        }
        _ => {
            let body = "Not Found";
            let response = format!(
                "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    }
}

/// Path to the dashboard PID file (`~/.minutes/dashboard.pid`).
pub fn dashboard_pid_path() -> PathBuf {
    Config::minutes_dir().join("dashboard.pid")
}

/// Start the dashboard HTTP server.
pub fn serve(config: &Config, port: u16, open_browser: bool) -> Result<()> {
    let pid_path = dashboard_pid_path();

    // Acquire PID guard (flock-based, auto-cleanup on exit)
    let _guard = match pid::create_pid_guard(&pid_path) {
        Ok(g) => g,
        Err(minutes_core::error::PidError::AlreadyRecording(existing_pid)) => {
            // Dashboard already running — print the URL and exit
            eprintln!(
                "Dashboard already running (PID {}). Open http://localhost:{} in your browser.",
                existing_pid, port
            );
            return Ok(());
        }
        Err(e) => return Err(e).context("failed to create dashboard PID file"),
    };

    // Try to bind to the requested port, fall back to OS-assigned
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port))
        .or_else(|_| {
            eprintln!("Port {} in use, using random port...", port);
            TcpListener::bind("127.0.0.1:0")
        })
        .context("failed to bind HTTP server")?;

    let actual_port = listener.local_addr()?.port();
    let url = format!("http://localhost:{}", actual_port);

    eprintln!("Minutes dashboard: {}", url);
    eprintln!("Press Ctrl+C to stop.");

    if open_browser {
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg(&url).spawn();
        }
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
        }
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", &url])
                .spawn();
        }
    }

    for stream in listener.incoming() {
        match stream {
            Ok(mut s) => handle_request(&mut s, config),
            Err(e) => {
                eprintln!("Connection error: {}", e);
            }
        }
    }

    Ok(())
}

// ── Tests ───────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_minutes() {
        assert_eq!(parse_duration("45m"), 2700);
    }

    #[test]
    fn parse_duration_hours_minutes() {
        assert_eq!(parse_duration("1h 12m"), 4320);
    }

    #[test]
    fn parse_duration_hms_colon() {
        assert_eq!(parse_duration("1:23:45"), 5025);
    }

    #[test]
    fn parse_duration_mm_ss() {
        assert_eq!(parse_duration("23:45"), 1425);
    }

    #[test]
    fn parse_duration_hours_only() {
        assert_eq!(parse_duration("2h"), 7200);
    }

    #[test]
    fn parse_duration_empty() {
        assert_eq!(parse_duration(""), 0);
    }

    #[test]
    fn parse_duration_garbage() {
        assert_eq!(parse_duration("garbage"), 0);
    }

    #[test]
    fn parse_duration_whitespace() {
        assert_eq!(parse_duration("  45m  "), 2700);
    }
}
