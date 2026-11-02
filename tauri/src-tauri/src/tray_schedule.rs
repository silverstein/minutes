//! The tray menu's calendar block: what is on now, what starts next, the rest
//! of today and tomorrow, grouped under section headers.
//!
//! Pure so the grouping and labels are testable; `main.rs` turns the rows
//! into menu items and maps clicks back to events by index.

use chrono::{DateTime, Duration, Local, NaiveDateTime, TimeZone, Timelike};
use minutes_core::calendar::CalendarEvent;
use minutes_core::i18n::tr;

/// Event rows shown at most, across all sections.
pub const MAX_EVENTS: usize = 10;
/// How far back to look for events still in progress (a long evening event
/// that started hours ago is still "now").
pub const LOOKBACK_MINUTES: u32 = 12 * 60;

const MAX_TITLE_CHARS: usize = 48;
const MAX_PLACE_CHARS: usize = 28;

/// An event's index into the input slice, with its parsed start and end.
type Slot = (usize, DateTime<Local>, Option<DateTime<Local>>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A disabled section label ("Now", "Starts in 6m", "Today", "Tomorrow").
    Header(String),
    /// A clickable event; `index` points into the events slice given to `rows`.
    Event { label: String, index: usize },
}

/// Minutes from `now` to the end of tomorrow.
pub fn lookahead_minutes(now: DateTime<Local>) -> u32 {
    let end = local_midnight(now, 2).unwrap_or(now + Duration::hours(48));
    (end - now).num_minutes().clamp(1, 3 * 24 * 60) as u32
}

pub fn rows(events: &[CalendarEvent], now: DateTime<Local>) -> Vec<Row> {
    let today = now.date_naive();
    let tomorrow = today + Duration::days(1);

    let mut current = Vec::new();
    let mut today_upcoming = Vec::new();
    let mut tomorrow_upcoming = Vec::new();
    for (index, event) in events.iter().enumerate() {
        let start = event_start(event, now);
        let end = event_end(event);
        if start <= now && end.is_some_and(|end| end > now) {
            current.push((index, start, end));
        } else if event.minutes_until >= 0 && start >= now - Duration::minutes(1) {
            if start.date_naive() == today {
                today_upcoming.push((index, start, end));
            } else if start.date_naive() == tomorrow {
                tomorrow_upcoming.push((index, start, end));
            }
        }
    }

    let mut out = Vec::new();
    let mut budget = MAX_EVENTS;
    let mut section = |out: &mut Vec<Row>, header: String, list: &[Slot]| {
        let take = list.len().min(budget);
        if take == 0 {
            return;
        }
        budget -= take;
        out.push(Row::Header(header));
        for &(index, start, end) in &list[..take] {
            out.push(Row::Event {
                label: label(&events[index], start, end),
                index,
            });
        }
    };

    section(&mut out, tr("Now").into_owned(), &current);
    if let Some((first, rest)) = today_upcoming.split_first() {
        section(
            &mut out,
            starts_in_header(events[first.0].minutes_until),
            std::slice::from_ref(first),
        );
        section(&mut out, tr("Today").into_owned(), rest);
    }
    section(&mut out, tr("Tomorrow").into_owned(), &tomorrow_upcoming);
    out
}

fn starts_in_header(minutes_until: i64) -> String {
    if minutes_until <= 0 {
        return tr("Starting now").into_owned();
    }
    let relative = if minutes_until < 60 {
        format!("{minutes_until}m")
    } else if minutes_until % 60 == 0 {
        format!("{}h", minutes_until / 60)
    } else {
        format!("{}h {}m", minutes_until / 60, minutes_until % 60)
    };
    tr("Starts in {}").replace("{}", &relative)
}

/// `Coffee: Samir / Mat · 9:30 - 10:30 AM · Betaworks, 29 Little West…`
fn label(event: &CalendarEvent, start: DateTime<Local>, end: Option<DateTime<Local>>) -> String {
    let mut label = format!(
        "{} · {}",
        truncate(event.title.trim(), MAX_TITLE_CHARS),
        time_range(start, end)
    );
    if let Some(place) = event.location.as_deref().and_then(place) {
        label.push_str(" · ");
        label.push_str(&truncate(&place, MAX_PLACE_CHARS));
    }
    label
}

fn time_range(start: DateTime<Local>, end: Option<DateTime<Local>>) -> String {
    let Some(end) = end.filter(|end| *end > start) else {
        return clock(start, true);
    };
    let same_half =
        start.date_naive() == end.date_naive() && (start.hour() < 12) == (end.hour() < 12);
    format!("{} - {}", clock(start, !same_half), clock(end, true))
}

fn clock(time: DateTime<Local>, with_meridiem: bool) -> String {
    if with_meridiem {
        time.format("%-I:%M %p").to_string()
    } else {
        time.format("%-I:%M").to_string()
    }
}

/// What to show for a location: the written place without any meeting links,
/// or the video service's name when the location is only a link.
fn place(location: &str) -> Option<String> {
    let text = location
        .split_whitespace()
        .filter(|word| !word.starts_with("http://") && !word.starts_with("https://"))
        .collect::<Vec<_>>()
        .join(" ");
    let text = text.trim_matches(|c: char| c.is_whitespace() || c == ',' || c == ';');
    if !text.is_empty() {
        return Some(text.to_string());
    }
    let lower = location.to_ascii_lowercase();
    [
        ("zoom.us", "Zoom"),
        ("meet.google.com", "Google Meet"),
        ("teams.microsoft.com", "Teams"),
        ("teams.live.com", "Teams"),
        ("webex.com", "Webex"),
    ]
    .iter()
    .find(|(host, _)| lower.contains(host))
    .map(|(_, name)| name.to_string())
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max - 1).collect();
    format!("{}…", kept.trim_end())
}

fn event_start(event: &CalendarEvent, now: DateTime<Local>) -> DateTime<Local> {
    parse_local(&event.start).unwrap_or_else(|| now + Duration::minutes(event.minutes_until))
}

fn event_end(event: &CalendarEvent) -> Option<DateTime<Local>> {
    event.end.as_deref().and_then(parse_local)
}

fn parse_local(value: &str) -> Option<DateTime<Local>> {
    let naive = NaiveDateTime::parse_from_str(value.trim(), "%Y-%m-%d %H:%M").ok()?;
    Local.from_local_datetime(&naive).earliest()
}

fn local_midnight(now: DateTime<Local>, days_ahead: i64) -> Option<DateTime<Local>> {
    let date = now.date_naive() + Duration::days(days_ahead);
    Local
        .from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
        .earliest()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(value: &str) -> DateTime<Local> {
        parse_local(value).unwrap()
    }

    fn event(title: &str, start: &str, end: Option<&str>, now: DateTime<Local>) -> CalendarEvent {
        CalendarEvent {
            title: title.to_string(),
            start: start.to_string(),
            minutes_until: (at(start) - now).num_minutes(),
            attendees: Vec::new(),
            url: None,
            calendar: None,
            end: end.map(str::to_string),
            location: None,
            uid: None,
        }
    }

    fn render(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Header(text) => format!("# {text}"),
                Row::Event { label, index } => format!("{index}: {label}"),
            })
            .collect()
    }

    #[test]
    fn groups_now_next_today_and_tomorrow() {
        let now = at("2026-10-30 09:24");
        let mut coffee = event(
            "Coffee: Samir / Mat (X1 Wealth)",
            "2026-10-30 09:30",
            Some("2026-10-30 10:30"),
            now,
        );
        coffee.location = Some("Betaworks, 29 Little West 12th St".to_string());
        let events = vec![
            event("Focus", "2026-10-30 09:00", Some("2026-10-30 09:45"), now),
            event("Ended", "2026-10-30 08:00", Some("2026-10-30 09:00"), now),
            coffee,
            event("buffer", "2026-10-30 10:30", Some("2026-10-30 11:15"), now),
            event(
                "NousCon 2026",
                "2026-10-30 18:00",
                Some("2026-10-31 00:00"),
                now,
            ),
            event("Hang", "2026-10-31 14:00", Some("2026-10-31 16:00"), now),
            event("Too far", "2026-11-01 09:00", Some("2026-11-01 10:00"), now),
        ];
        assert_eq!(
            render(&rows(&events, now)),
            vec![
                "# Now",
                "0: Focus · 9:00 - 9:45 AM",
                "# Starts in 6m",
                "2: Coffee: Samir / Mat (X1 Wealth) · 9:30 - 10:30 AM · Betaworks, 29 Little West 1…",
                "# Today",
                "3: buffer · 10:30 - 11:15 AM",
                "4: NousCon 2026 · 6:00 PM - 12:00 AM",
                "# Tomorrow",
                "5: Hang · 2:00 - 4:00 PM",
            ]
        );
    }

    #[test]
    fn next_event_tomorrow_skips_the_starts_in_header() {
        let now = at("2026-10-30 22:00");
        let events = vec![event("Hang", "2026-10-31 14:00", None, now)];
        assert_eq!(
            render(&rows(&events, now)),
            vec!["# Tomorrow", "0: Hang · 2:00 PM"]
        );
    }

    #[test]
    fn without_an_end_time_a_started_event_drops_off() {
        // The AppleScript fallback reports no end, so "in progress" is unknowable.
        let now = at("2026-10-30 09:24");
        let events = vec![
            event("Started", "2026-10-30 09:00", None, now),
            event("Starting", "2026-10-30 09:24", None, now),
        ];
        assert_eq!(
            render(&rows(&events, now)),
            vec!["# Starting now", "1: Starting · 9:24 AM"]
        );
    }

    #[test]
    fn caps_event_rows() {
        let now = at("2026-10-30 08:00");
        let events: Vec<_> = (0..14)
            .map(|i| {
                let start = format!("2026-10-30 {:02}:00", 9 + i % 12);
                event(&format!("E{i}"), &start, None, now)
            })
            .collect();
        let shown = rows(&events, now)
            .iter()
            .filter(|row| matches!(row, Row::Event { .. }))
            .count();
        assert_eq!(shown, MAX_EVENTS);
    }

    #[test]
    fn starts_in_header_formats_hours() {
        assert_eq!(starts_in_header(6), "Starts in 6m");
        assert_eq!(starts_in_header(60), "Starts in 1h");
        assert_eq!(starts_in_header(65), "Starts in 1h 5m");
        assert_eq!(starts_in_header(0), "Starting now");
    }

    #[test]
    fn place_drops_links_and_names_video_services() {
        assert_eq!(
            place("https://us02web.zoom.us/j/123").as_deref(),
            Some("Zoom")
        );
        assert_eq!(
            place("https://meet.google.com/abc").as_deref(),
            Some("Google Meet")
        );
        assert_eq!(
            place("Room 4, https://meet.google.com/abc").as_deref(),
            Some("Room 4")
        );
        assert_eq!(place("https://example.com/x"), None);
        assert_eq!(place("WSA").as_deref(), Some("WSA"));
    }

    #[test]
    fn lookahead_reaches_the_end_of_tomorrow() {
        let now = at("2026-10-30 09:24");
        assert_eq!(
            lookahead_minutes(now),
            (at("2026-11-01 00:00") - now).num_minutes() as u32
        );
    }
}
