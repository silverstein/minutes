#!/usr/bin/env swift
// Queries macOS Calendar via EventKit and prints upcoming events as JSON lines.
// Usage: swift calendar-events.swift [lookahead_minutes] [lookback_minutes] [reference_epoch_seconds]
// Output: one JSON object per line:
//   {"title":"...","start":"...","minutes_until":N,"attendees":["..."],"url":"..."}
//
// When lookback_minutes is provided, queries from (now - lookback) to (now + lookahead).
// This supports overlap queries for matching recordings to calendar events.
// When reference_epoch_seconds is provided, the query is centered on that timestamp
// instead of wall-clock now. This keeps delayed reprocessing aligned to recording time.

import EventKit
import Foundation

struct EventOutput: Codable {
    let title: String
    let start: String
    let minutes_until: Int
    let attendees: [String]
    let url: String?
    let calendar: String?
    // End time and the iCalendar UID let callers show events in progress and
    // collapse one invite seen through two accounts into a single entry.
    let end: String?
    let uid: String?
}

func printAccessStatus() {
    let label: String
    switch EKEventStore.authorizationStatus(for: .event) {
    case .fullAccess: label = "full-access"
    case .writeOnly: label = "write-only"
    case .denied: label = "denied"
    case .restricted: label = "restricted"
    case .notDetermined: label = "not-determined"
    @unknown default: label = "unknown"
    }
    print("{\"calendar_access\":\"\(label)\"}")
}

// The desktop app requests consent. Status and event reads never prompt.
if CommandLine.arguments.dropFirst().first == "--status" {
    printAccessStatus()
    exit(0)
}

guard EKEventStore.authorizationStatus(for: .event) == .fullAccess else {
    FileHandle.standardError.write("calendar-events: full calendar access is required\n".data(using: .utf8)!)
    exit(2)
}

let lookaheadMinutes = Int(CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "240") ?? 240
let lookbackMinutes = Int(CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "0") ?? 0
let referenceEpochSeconds = CommandLine.arguments.count > 3 ? Double(CommandLine.arguments[3]) : nil
let store = EKEventStore()
let encoder = JSONEncoder()
encoder.outputFormatting = [] // compact, single-line

func printEvents() {
    let now = referenceEpochSeconds.map(Date.init(timeIntervalSince1970:)) ?? Date()
    guard let end = Calendar.current.date(byAdding: .minute, value: lookaheadMinutes, to: now) else { return }
    let start: Date
    if lookbackMinutes > 0 {
        guard let s = Calendar.current.date(byAdding: .minute, value: -lookbackMinutes, to: now) else { return }
        start = s
    } else {
        start = now
    }
    let predicate = store.predicateForEvents(withStart: start, end: end, calendars: nil)
    let events = store.events(matching: predicate)
        .filter { !$0.isAllDay }
        .sorted { $0.startDate < $1.startDate }

    // Fixed Gregorian/POSIX output: callers parse these as wall-clock times,
    // and a Buddhist or Japanese system calendar would shift the year.
    let formatter = DateFormatter()
    formatter.locale = Locale(identifier: "en_US_POSIX")
    formatter.calendar = Calendar(identifier: .gregorian)
    formatter.dateFormat = "yyyy-MM-dd HH:mm"

    for event in events {
        let mins = Int(event.startDate.timeIntervalSince(now) / 60)
        let startStr = formatter.string(from: event.startDate)
        let title = event.title ?? "Untitled"

        var attendeeNames: [String] = []
        if let attendees = event.attendees {
            for attendee in attendees {
                if let name = attendee.name {
                    attendeeNames.append(name)
                }
            }
        }

        let output = EventOutput(
            title: title,
            start: startStr,
            minutes_until: mins,
            attendees: attendeeNames,
            url: event.location,
            calendar: event.calendar?.title,
            end: event.endDate.map { formatter.string(from: $0) },
            uid: event.calendarItemExternalIdentifier
        )

        if let data = try? encoder.encode(output), let line = String(data: data, encoding: .utf8) {
            print(line)
        }
    }
}

printEvents()
