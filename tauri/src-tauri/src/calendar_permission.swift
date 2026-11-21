import EventKit
import Foundation

private var pendingCalendarRequest: EKEventStore?

/// Invoked on the desktop main thread by the Calendar access action.
/// Returns false when Calendar access is already decided.
@_cdecl("minutes_request_calendar_access")
public func minutesRequestCalendarAccess() -> Bool {
    guard #available(macOS 14.0, *),
          EKEventStore.authorizationStatus(for: .event) == .notDetermined else {
        return false
    }
    guard pendingCalendarRequest == nil else { return true }

    let store = EKEventStore()
    pendingCalendarRequest = store
    store.requestFullAccessToEvents { _, _ in
        DispatchQueue.main.async {
            pendingCalendarRequest = nil
        }
    }
    return true
}
