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
}

// `--status` prints the current authorization state WITHOUT prompting, so
// Settings/readiness can be truthful without triggering a permission dialog.
// States: full-access | write-only | denied | restricted | not-determined
if CommandLine.arguments.count > 1 && CommandLine.arguments[1] == "--status" {
    let status = EKEventStore.authorizationStatus(for: .event)
    let label: String
    switch status {
    case .fullAccess: label = "full-access"
    case .writeOnly: label = "write-only"
    case .denied: label = "denied"
    case .restricted: label = "restricted"
    case .notDetermined: label = "not-determined"
    @unknown default: label = "unknown"
    }
    print("{\"calendar_access\":\"\(label)\"}")
    exit(0)
}

let lookaheadMinutes = Int(CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "240") ?? 240
let lookbackMinutes = Int(CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "0") ?? 0
let referenceEpochSeconds = CommandLine.arguments.count > 3 ? Double(CommandLine.arguments[3]) : nil
let store = EKEventStore()
let semaphore = DispatchSemaphore(value: 0)

let encoder = JSONEncoder()
encoder.outputFormatting = [] // compact, single-line

var accessDenied = false
store.requestFullAccessToEvents { granted, error in
    defer { semaphore.signal() }
    guard granted else {
        // Distinguish denied/write-only from "no events": empty-success here
        // made dead reminders unreadable from the outside (#300).
        accessDenied = true
        return
    }

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

    let formatter = DateFormatter()
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
            url: event.location
        )

        if let data = try? encoder.encode(output), let line = String(data: data, encoding: .utf8) {
            print(line)
        }
    }
}

semaphore.wait()
if accessDenied {
    FileHandle.standardError.write("calendar-events: access not granted (full access required)\n".data(using: .utf8)!)
    exit(2)
}
