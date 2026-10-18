#!/usr/bin/env python3
"""Compile and exercise the shipped Swift device/attribution boundary on macOS."""
import pathlib
import subprocess
import sys
import tempfile

if sys.platform != "darwin":
    print("mic_check Swift tests require macOS")
    sys.exit(0)

source = pathlib.Path(__file__).resolve().parents[1] / "tauri/src-tauri/src/mic_check.swift"
with tempfile.TemporaryDirectory(prefix="minutes-mic-check-") as directory:
    root = pathlib.Path(directory)
    main = root / "main.swift"
    main.write_text('''import Foundation
var deviceReads = 0
var processReads = 0
func sample(_ device: Bool?, _ pids: [pid_t]?) -> [String: Any] {
    deviceReads = 0
    processReads = 0
    return callInputState(deviceProbe: { deviceReads += 1; return device },
                          attributionProbe: { processReads += 1; return pids })
}
let idle = sample(false, [123])
assert(deviceReads == 1 && processReads == 0, "idle devices must never enumerate processes")
assert(idle["mic_live"] as? Bool == false)
assert((idle["active_input_pids"] as? [pid_t])?.isEmpty == true)
let active = sample(true, [123])
assert(deviceReads == 1 && processReads == 1)
assert(active["mic_live"] as? Bool == true)
assert(active["active_input_pids"] as? [pid_t] == [123])
let unknownDevice = sample(nil, [456])
assert(processReads == 1, "an unreadable route must not suppress attribution")
assert(unknownDevice["device_active"] is NSNull)
assert(unknownDevice["active_input_pids"] as? [pid_t] == [456])
let unavailableAttribution = sample(true, nil)
assert(processReads == 1, "failed attribution must not be retried")
assert(unavailableAttribution["active_input_pids"] is NSNull)
assert(unavailableAttribution["mic_live"] as? Bool == true)
let emptyAttribution = sample(true, [])
assert(emptyAttribution["mic_live"] as? Bool == false, "device activity alone must not invent process input")
let unknown = sample(nil, nil)
assert(unknown["active_input_pids"] is NSNull)
assert(unknown["device_active"] is NSNull)
assert(unknown["mic_live"] as? Bool == false)
print("mic_check: 6 device/attribution boundary cases passed")
''')
    main.write_text(source.read_text() + "\n" + main.read_text())
    binary = root / "mic-check-test"
    subprocess.run(["swiftc", "-D", "MIC_CHECK_LIBRARY", str(main), "-o", str(binary)], check=True)
    subprocess.run([str(binary)], check=True)
