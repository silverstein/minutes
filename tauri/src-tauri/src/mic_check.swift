// Minimal Swift helper to check if any audio input is active.
// Default output is "1" if input capture is active, "0" if idle.
// --call-input-state returns a device-gated attribution snapshot as JSON.

import CoreAudio
import Foundation

let systemObject = AudioObjectID(kAudioObjectSystemObject)

func objectIDs(
    for selector: AudioObjectPropertySelector,
    on objectID: AudioObjectID = systemObject,
    scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal
) -> [AudioObjectID]? {
    var address = AudioObjectPropertyAddress(
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain
    )
    var size: UInt32 = 0
    guard AudioObjectGetPropertyDataSize(objectID, &address, 0, nil, &size) == noErr else {
        return nil
    }
    let count = Int(size) / MemoryLayout<AudioObjectID>.size
    guard count > 0 else {
        return []
    }

    var ids = [AudioObjectID](repeating: AudioObjectID(kAudioObjectUnknown), count: count)
    let status = ids.withUnsafeMutableBufferPointer { buffer in
        guard let baseAddress = buffer.baseAddress else {
            return kAudioHardwareBadObjectError
        }
        return AudioObjectGetPropertyData(objectID, &address, 0, nil, &size, baseAddress)
    }
    guard status == noErr else {
        return nil
    }
    return ids
}

func uint32Property(
    _ selector: AudioObjectPropertySelector,
    on objectID: AudioObjectID,
    scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal
) -> UInt32? {
    var value: UInt32 = 0
    var size = UInt32(MemoryLayout<UInt32>.size)
    var address = AudioObjectPropertyAddress(
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain
    )
    guard AudioObjectGetPropertyData(objectID, &address, 0, nil, &size, &value) == noErr else {
        return nil
    }
    return value
}

func pidProperty(on objectID: AudioObjectID) -> pid_t? {
    var value = pid_t(0)
    var size = UInt32(MemoryLayout<pid_t>.size)
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioProcessPropertyPID,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
    guard AudioObjectGetPropertyData(objectID, &address, 0, nil, &size, &value) == noErr else {
        return nil
    }
    return value
}

func activeInputProcessIDs() -> [pid_t]? {
    guard let processes = objectIDs(for: kAudioHardwarePropertyProcessObjectList) else {
        return nil
    }
    var pids: [pid_t] = []
    var sawReadableProcess = false
    for processID in processes {
        guard let isRunning = uint32Property(kAudioProcessPropertyIsRunningInput, on: processID) else {
            continue
        }
        sawReadableProcess = true
        if isRunning > 0 {
            if let pid = pidProperty(on: processID), pid > 0 {
                pids.append(pid)
            }
        }
    }
    return sawReadableProcess ? pids : nil
}

func inputChannelCount(for deviceID: AudioObjectID) -> UInt32? {
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioDevicePropertyStreamConfiguration,
        mScope: kAudioDevicePropertyScopeInput,
        mElement: kAudioObjectPropertyElementMain
    )
    var size: UInt32 = 0
    guard AudioObjectGetPropertyDataSize(deviceID, &address, 0, nil, &size) == noErr else {
        return nil
    }
    guard size > 0 else { return 0 }

    let bufferList = UnsafeMutableRawPointer.allocate(
        byteCount: Int(size),
        alignment: MemoryLayout<AudioBufferList>.alignment
    )
    defer { bufferList.deallocate() }

    guard AudioObjectGetPropertyData(deviceID, &address, 0, nil, &size, bufferList) == noErr else {
        return nil
    }

    let audioBufferList = bufferList.assumingMemoryBound(to: AudioBufferList.self)
    return UnsafeMutableAudioBufferListPointer(audioBufferList)
        .reduce(UInt32(0)) { total, buffer in total + buffer.mNumberChannels }
}

// Unknown device state must not suppress process attribution: a route can
// disappear during enumeration, and a non-default input may still be active.
func anyInputDeviceRunning() -> Bool? {
    guard let devices = objectIDs(for: kAudioHardwarePropertyDevices) else {
        return nil
    }
    var unknown = false
    for deviceID in devices {
        guard let channels = inputChannelCount(for: deviceID) else {
            unknown = true
            continue
        }
        if channels == 0 { continue }
        guard let running = uint32Property(kAudioDevicePropertyDeviceIsRunningSomewhere, on: deviceID) else {
            unknown = true
            continue
        }
        if running > 0 { return true }
    }
    return unknown ? nil : false
}

func callInputState(
    deviceProbe: () -> Bool? = anyInputDeviceRunning,
    attributionProbe: () -> [pid_t]? = activeInputProcessIDs
) -> [String: Any] {
    let deviceActive = deviceProbe()
    // This branch never enumerates process objects when every input is idle.
    let pids = deviceActive == false ? [] : attributionProbe()
    let micLive = pids.map { !$0.isEmpty } ?? deviceActive ?? false
    return [
        "mic_live": micLive,
        "device_active": deviceActive.map { $0 as Any } ?? NSNull(),
        "active_input_pids": pids.map { $0 as Any } ?? NSNull(),
    ]
}

#if !MIC_CHECK_LIBRARY
// Keep the CoreAudio client alive between polls. Starting a new client for
// each device-only query can itself provoke repeated TCC checks (#1127).
if CommandLine.arguments.contains("--serve-call-input") {
    while let command = readLine() {
        if command == "full" {
            let data = try JSONSerialization.data(withJSONObject: callInputState(), options: [.sortedKeys])
            print(String(decoding: data, as: UTF8.self))
        } else if command == "device" {
            print(anyInputDeviceRunning().map { $0 ? "1" : "0" } ?? "unknown")
        } else {
            exit(2)
        }
        fflush(stdout)
    }
    exit(0)
}

if CommandLine.arguments.contains("--device-input-active") {
    guard let active = anyInputDeviceRunning() else { exit(2) }
    print(active ? "1" : "0")
    exit(0)
}

if CommandLine.arguments.contains("--call-input-state") {
    let data = try JSONSerialization.data(withJSONObject: callInputState(), options: [.sortedKeys])
    print(String(decoding: data, as: UTF8.self))
    exit(0)
}

if CommandLine.arguments.contains("--active-input-pids") {
    guard let pids = activeInputProcessIDs() else {
        exit(2)
    }
    for pid in pids {
        print(pid)
    }
    exit(0)
}

let micActive = activeInputProcessIDs().map { !$0.isEmpty } ?? anyInputDeviceRunning() ?? false
print(micActive ? "1" : "0")

#endif
