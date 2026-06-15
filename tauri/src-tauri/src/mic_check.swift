// Minimal Swift helper to check if any audio input is active.
// Outputs "1" if mic/input capture is active, "0" if idle.

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

func inputChannelCount(for deviceID: AudioObjectID) -> UInt32 {
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioDevicePropertyStreamConfiguration,
        mScope: kAudioDevicePropertyScopeInput,
        mElement: kAudioObjectPropertyElementMain
    )
    var size: UInt32 = 0
    guard AudioObjectGetPropertyDataSize(deviceID, &address, 0, nil, &size) == noErr, size > 0 else {
        return 0
    }

    let bufferList = UnsafeMutableRawPointer.allocate(
        byteCount: Int(size),
        alignment: MemoryLayout<AudioBufferList>.alignment
    )
    defer { bufferList.deallocate() }

    guard AudioObjectGetPropertyData(deviceID, &address, 0, nil, &size, bufferList) == noErr else {
        return 0
    }

    let audioBufferList = bufferList.assumingMemoryBound(to: AudioBufferList.self)
    return UnsafeMutableAudioBufferListPointer(audioBufferList)
        .reduce(UInt32(0)) { total, buffer in total + buffer.mNumberChannels }
}

func anyInputDeviceRunning() -> Bool {
    guard let devices = objectIDs(for: kAudioHardwarePropertyDevices) else {
        return false
    }
    return devices.contains { deviceID in
        inputChannelCount(for: deviceID) > 0
            && (uint32Property(kAudioDevicePropertyDeviceIsRunningSomewhere, on: deviceID) ?? 0) > 0
    }
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

let micActive = activeInputProcessIDs().map { !$0.isEmpty } ?? anyInputDeviceRunning()
print(micActive ? "1" : "0")
