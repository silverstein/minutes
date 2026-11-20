import EventKit
import Foundation

/// Invoked on the desktop app's main thread after startup.
@_cdecl("minutes_request_calendar_access")
public func minutesRequestCalendarAccess() {
    guard #available(macOS 14.0, *),
          EKEventStore.authorizationStatus(for: .event) == .notDetermined else {
        return
    }

    let store = EKEventStore()
    store.requestFullAccessToEvents { [store] _, _ in
        withExtendedLifetime(store) {}
    }
}
