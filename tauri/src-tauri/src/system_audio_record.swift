import AVFoundation
import AudioToolbox
import CoreMedia
import Dispatch
import Foundation
import ScreenCaptureKit

private func availableMicrophones() -> [AVCaptureDevice] {
    AVCaptureDevice.DiscoverySession(
        deviceTypes: [.microphone],
        mediaType: .audio,
        position: .unspecified
    ).devices
}

private func emitJSON(_ payload: [String: Any]) {
    if let data = try? JSONSerialization.data(withJSONObject: payload),
       let json = String(data: data, encoding: .utf8) {
        print(json)
        fflush(stdout)
    }
}

private func formatIDString(_ formatID: AudioFormatID) -> String {
    let bytes: [UInt8] = [
        UInt8((formatID >> 24) & 0xff),
        UInt8((formatID >> 16) & 0xff),
        UInt8((formatID >> 8) & 0xff),
        UInt8(formatID & 0xff),
    ]
    return String(bytes: bytes, encoding: .ascii) ?? String(formatID)
}

private struct LinearPCMLayout {
    let channels: Int
    let bitsPerChannel: Int
    let bytesPerFrame: Int
    let bytesPerSample: Int
    let isFloat: Bool
    let isSignedInteger: Bool
    let isBigEndian: Bool
    let isNonInterleaved: Bool
    let isAlignedHigh: Bool

    init?(_ asbd: AudioStreamBasicDescription) {
        guard asbd.mFormatID == kAudioFormatLinearPCM else { return nil }

        let channels = Int(asbd.mChannelsPerFrame)
        let bitsPerChannel = Int(asbd.mBitsPerChannel)
        let bytesPerFrame = Int(asbd.mBytesPerFrame)
        guard channels > 0,
              bitsPerChannel > 0,
              bitsPerChannel <= 64,
              bytesPerFrame > 0 else {
            return nil
        }

        let flags = asbd.mFormatFlags
        let isNonInterleaved = (flags & kAudioFormatFlagIsNonInterleaved) != 0
        let bytesPerSample: Int
        if isNonInterleaved {
            bytesPerSample = bytesPerFrame
        } else {
            guard bytesPerFrame % channels == 0 else { return nil }
            bytesPerSample = bytesPerFrame / channels
        }
        guard bytesPerSample > 0,
              bytesPerSample <= 8,
              bitsPerChannel <= bytesPerSample * 8 else {
            return nil
        }

        self.channels = channels
        self.bitsPerChannel = bitsPerChannel
        self.bytesPerFrame = bytesPerFrame
        self.bytesPerSample = bytesPerSample
        self.isFloat = (flags & kAudioFormatFlagIsFloat) != 0
        self.isSignedInteger = (flags & kAudioFormatFlagIsSignedInteger) != 0
        self.isBigEndian = (flags & kAudioFormatFlagIsBigEndian) != 0
        self.isNonInterleaved = isNonInterleaved
        self.isAlignedHigh = (flags & kAudioFormatFlagIsAlignedHigh) != 0
    }

    func sampleOffset(frame: Int, channel: Int, frameCount: Int) -> Int {
        if isNonInterleaved {
            return channel * frameCount * bytesPerFrame + frame * bytesPerFrame
        }
        return frame * bytesPerFrame + channel * bytesPerSample
    }

    func decodeSample(_ bytes: UnsafeRawBufferPointer, offset: Int) -> Float? {
        guard offset >= 0, offset + bytesPerSample <= bytes.count else { return nil }

        var raw: UInt64 = 0
        if isBigEndian {
            for index in 0..<bytesPerSample {
                raw = (raw << 8) | UInt64(bytes[offset + index])
            }
        } else {
            for index in 0..<bytesPerSample {
                raw |= UInt64(bytes[offset + index]) << UInt64(index * 8)
            }
        }

        let containerBits = bytesPerSample * 8
        if isAlignedHigh, containerBits > bitsPerChannel {
            raw >>= UInt64(containerBits - bitsPerChannel)
        }

        if isFloat {
            let sample: Double
            switch bitsPerChannel {
            case 32:
                sample = Double(Float(bitPattern: UInt32(truncatingIfNeeded: raw)))
            case 64:
                sample = Double(bitPattern: raw)
            default:
                return nil
            }
            return sample.isFinite ? Float(sample) : 0
        }

        let mask: UInt64 = bitsPerChannel == 64
            ? UInt64.max
            : (UInt64(1) << UInt64(bitsPerChannel)) - 1
        raw &= mask

        if isSignedInteger {
            let signBit = UInt64(1) << UInt64(bitsPerChannel - 1)
            let signedValue: Int64
            if bitsPerChannel == 64 {
                signedValue = Int64(bitPattern: raw)
            } else if (raw & signBit) != 0 {
                signedValue = Int64(bitPattern: raw | ~mask)
            } else {
                signedValue = Int64(raw)
            }
            let scale = pow(2.0, Double(bitsPerChannel - 1))
            return Float(Double(signedValue) / scale)
        }

        let midpoint = Double(mask) / 2.0
        guard midpoint > 0 else { return nil }
        return Float((Double(raw) - midpoint) / midpoint)
    }
}

private func decodeLinearPCMToMono(
    bytes: UnsafeRawBufferPointer,
    frameCount: Int,
    asbd: AudioStreamBasicDescription,
    output: UnsafeMutablePointer<Float>
) -> Bool {
    guard frameCount > 0, let layout = LinearPCMLayout(asbd) else { return false }

    for frame in 0..<frameCount {
        var sum: Float = 0
        for channel in 0..<layout.channels {
            let offset = layout.sampleOffset(
                frame: frame,
                channel: channel,
                frameCount: frameCount
            )
            guard let sample = layout.decodeSample(bytes, offset: offset) else {
                return false
            }
            sum += sample
        }
        output[frame] = sum / Float(layout.channels)
    }
    return true
}

private func testASBD(
    channels: UInt32,
    bitsPerChannel: UInt32,
    bytesPerFrame: UInt32,
    flags: AudioFormatFlags
) -> AudioStreamBasicDescription {
    var asbd = AudioStreamBasicDescription()
    asbd.mSampleRate = 16_000
    asbd.mFormatID = kAudioFormatLinearPCM
    asbd.mFormatFlags = flags
    asbd.mBytesPerPacket = bytesPerFrame
    asbd.mFramesPerPacket = 1
    asbd.mBytesPerFrame = bytesPerFrame
    asbd.mChannelsPerFrame = channels
    asbd.mBitsPerChannel = bitsPerChannel
    return asbd
}

private func appendLittleEndian<T: FixedWidthInteger>(_ value: T, to bytes: inout [UInt8]) {
    var littleEndian = value.littleEndian
    withUnsafeBytes(of: &littleEndian) { bytes.append(contentsOf: $0) }
}

private func appendFloat32(_ value: Float, to bytes: inout [UInt8]) {
    appendLittleEndian(value.bitPattern, to: &bytes)
}

private func decodedSamples(
    bytes: [UInt8],
    frameCount: Int,
    asbd: AudioStreamBasicDescription
) -> [Float]? {
    var output = [Float](repeating: 0, count: frameCount)
    let decoded = bytes.withUnsafeBytes { input in
        output.withUnsafeMutableBufferPointer { destination in
            guard let baseAddress = destination.baseAddress else { return false }
            return decodeLinearPCMToMono(
                bytes: input,
                frameCount: frameCount,
                asbd: asbd,
                output: baseAddress
            )
        }
    }
    return decoded ? output : nil
}

private func runPCMDecoderSelfTest() -> Bool {
    let signedPacked = kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked
    let signedHighAligned = kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsAlignedHigh

    var int16Bytes: [UInt8] = []
    appendLittleEndian(Int16(16_384), to: &int16Bytes)
    appendLittleEndian(Int16(-16_384), to: &int16Bytes)
    let int16 = decodedSamples(
        bytes: int16Bytes,
        frameCount: 2,
        asbd: testASBD(channels: 1, bitsPerChannel: 16, bytesPerFrame: 2, flags: signedPacked)
    )

    // Bluetooth/HFP paths can expose 16 significant bits high-aligned inside
    // a 32-bit Core Audio container. The old decoder read the low padding as
    // Int16, turning valid speech into a near-silent stem (issue #814).
    var highAlignedBytes: [UInt8] = []
    appendLittleEndian(Int32(16_384) << 16, to: &highAlignedBytes)
    appendLittleEndian(Int32(-16_384) << 16, to: &highAlignedBytes)
    let highAligned = decodedSamples(
        bytes: highAlignedBytes,
        frameCount: 2,
        asbd: testASBD(
            channels: 1,
            bitsPerChannel: 16,
            bytesPerFrame: 4,
            flags: signedHighAligned
        )
    )

    var int32Bytes: [UInt8] = []
    appendLittleEndian(Int32(1_073_741_824), to: &int32Bytes)
    appendLittleEndian(Int32(-1_073_741_824), to: &int32Bytes)
    let int32 = decodedSamples(
        bytes: int32Bytes,
        frameCount: 2,
        asbd: testASBD(channels: 1, bitsPerChannel: 32, bytesPerFrame: 4, flags: signedPacked)
    )

    var stereoFloatBytes: [UInt8] = []
    appendFloat32(0.75, to: &stereoFloatBytes)
    appendFloat32(0.25, to: &stereoFloatBytes)
    appendFloat32(-0.75, to: &stereoFloatBytes)
    appendFloat32(-0.25, to: &stereoFloatBytes)
    let stereoFloat = decodedSamples(
        bytes: stereoFloatBytes,
        frameCount: 2,
        asbd: testASBD(
            channels: 2,
            bitsPerChannel: 32,
            bytesPerFrame: 8,
            flags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked
        )
    )

    let tolerance: Float = 0.000_01
    let cases = [int16, highAligned, int32, stereoFloat]
    let passed = cases.allSatisfy { samples in
        guard let samples, samples.count == 2 else { return false }
        return abs(samples[0] - 0.5) < tolerance && abs(samples[1] + 0.5) < tolerance
    }
    if passed {
        emitJSON(["event": "pcm_decoder_self_test", "status": "ok", "cases": cases.count])
    } else {
        fputs("PCM decoder self-test failed\n", stderr)
    }
    return passed
}

@available(macOS 15.0, *)
final class NativeCallRecorder: NSObject, SCRecordingOutputDelegate, SCStreamOutput {
    private var stream: SCStream?
    private var recordingOutput: SCRecordingOutput?
    private let outputURL: URL
    private let requestedMicrophoneName: String?
    private var selectedMicrophoneID: String?
    private var selectedMicrophoneName: String?
    private var selectedMicrophoneDeviceSampleRate: Double?
    private var microphoneSelectionEvent: [String: Any]?
    private let sampleQueue = DispatchQueue(label: "minutes.system-audio.samples")
    private var monitorTimer: DispatchSourceTimer?
    private var lastSystemAudioSampleAt: Date?
    private var lastMicSampleAt: Date?
    private var lastReportedSystemLive = false
    private var lastReportedMicLive = false
    private var latestSystemLevel: UInt32 = 0
    private var latestMicLevel: UInt32 = 0
    private var protocolReady = false
    private var pendingSourceEvents: [[String: Any]] = []
    private var reportedSourceFormats = Set<String>()
    private var reportedDecodeFailures = Set<String>()

    // Per-source stem writers
    private var voiceStemFile: AVAudioFile?
    private var systemStemFile: AVAudioFile?
    // Set once stop() has closed the stems, and never cleared. Both stem files
    // are created lazily on first sample, so without this a sample arriving
    // after the close reopens the file with AVAudioFile(forWriting:), which
    // truncates it. Closing the stems does not stop the stream: stopCapture()
    // is awaited afterwards and ScreenCaptureKit keeps delivering until it
    // returns, so that window is not rare, it is every stop where remote audio
    // is still playing. Mutated and read only on sampleQueue (issue #792).
    private var stemsClosed = false
    private var voiceStemURL: URL?
    private var systemStemURL: URL?

    // Finalize start timestamp, set when stop() begins. Read by
    // recordingOutputDidFinishRecording to emit the final
    // `finalize_complete` event with elapsed_ms so we can characterize
    // the stopCapture-to-finalize curve on long captures (issue #236
    // follow-on to #216).
    private var finalizeStart: Date?

    init(outputURL: URL, requestedMicrophoneName: String?) {
        self.outputURL = outputURL
        self.requestedMicrophoneName = requestedMicrophoneName
    }

    deinit {
        NotificationCenter.default.removeObserver(self)
    }

    func start() async throws {
        let shareableContent = try await SCShareableContent.excludingDesktopWindows(
            false,
            onScreenWindowsOnly: true
        )
        guard let display = shareableContent.displays.first else {
            throw NSError(
                domain: "MinutesSystemAudioRecord",
                code: 1,
                userInfo: [NSLocalizedDescriptionKey: "No display available for ScreenCaptureKit capture."]
            )
        }

        let filter = SCContentFilter(
            display: display,
            excludingApplications: [],
            exceptingWindows: []
        )

        let configuration = SCStreamConfiguration()
        configuration.width = 2
        configuration.height = 2
        configuration.minimumFrameInterval = CMTime(value: 1, timescale: 2)
        configuration.queueDepth = 3
        configuration.capturesAudio = true
        configuration.captureMicrophone = true
        configuration.excludesCurrentProcessAudio = true
        configuration.showsCursor = false

        let microphone: AVCaptureDevice?
        if let requestedMicrophoneName {
            microphone = availableMicrophones().first {
                $0.localizedName == requestedMicrophoneName
            } ?? AVCaptureDevice.default(for: .audio)
            if microphone?.localizedName == requestedMicrophoneName {
                microphoneSelectionEvent = [
                    "event": "microphone_selected",
                    "name": requestedMicrophoneName,
                    "configured": true,
                ]
            } else {
                // The Rust parent preflights the same exact-name lookup. This
                // branch covers the device disappearing in the small race
                // between preflight and stream configuration. It must be
                // reported only after `ready` because the first stdout line is
                // the helper protocol handshake.
                microphoneSelectionEvent = [
                    "event": "microphone_fallback",
                    "name": requestedMicrophoneName,
                    "message": "configured mic not found, using default",
                ]
            }
        } else {
            microphone = AVCaptureDevice.default(for: .audio)
        }

        if let microphone {
            configuration.microphoneCaptureDeviceID = microphone.uniqueID
            selectedMicrophoneID = microphone.uniqueID
            selectedMicrophoneName = microphone.localizedName
            if let deviceASBD = CMAudioFormatDescriptionGetStreamBasicDescription(
                microphone.activeFormat.formatDescription
            )?.pointee {
                selectedMicrophoneDeviceSampleRate = deviceASBD.mSampleRate
            }
            if requestedMicrophoneName != nil {
                NotificationCenter.default.addObserver(
                    self,
                    selector: #selector(microphoneWasDisconnected(_:)),
                    name: AVCaptureDevice.wasDisconnectedNotification,
                    object: microphone
                )
            }
        }

        let stream = SCStream(filter: filter, configuration: configuration, delegate: nil)
        try stream.addStreamOutput(self, type: .audio, sampleHandlerQueue: sampleQueue)
        try stream.addStreamOutput(self, type: .microphone, sampleHandlerQueue: sampleQueue)
        let recordingConfiguration = SCRecordingOutputConfiguration()
        recordingConfiguration.outputURL = outputURL
        recordingConfiguration.outputFileType = .mov
        recordingConfiguration.videoCodecType = .h264

        let recordingOutput = SCRecordingOutput(
            configuration: recordingConfiguration,
            delegate: self
        )

        try stream.addRecordingOutput(recordingOutput)

        // Derive stem paths BEFORE startCapture to avoid race with early samples
        let baseName = outputURL.deletingPathExtension().lastPathComponent
        let stemDir = outputURL.deletingLastPathComponent()
        voiceStemURL = stemDir.appendingPathComponent("\(baseName).voice.wav")
        systemStemURL = stemDir.appendingPathComponent("\(baseName).system.wav")

        try await stream.startCapture()

        startMonitoring()

        self.stream = stream
        self.recordingOutput = recordingOutput
    }

    @objc private func microphoneWasDisconnected(_ notification: Notification) {
        guard let device = notification.object as? AVCaptureDevice,
              device.uniqueID == selectedMicrophoneID else {
            return
        }
        let payload: [String: Any] = [
            "event": "microphone_disconnected",
            "name": selectedMicrophoneName ?? device.localizedName,
        ]
        if let data = try? JSONSerialization.data(withJSONObject: payload),
           let json = String(data: data, encoding: .utf8) {
            print(json)
            fflush(stdout)
        }
    }

    func stop() async {
        // Spin up the finalize heartbeat BEFORE the sampleQueue.sync block so
        // the Rust parent sees stdout activity throughout the entire stop
        // sequence. Long captures (1h+) take tens of seconds to write the
        // moov atom inside `stream.stopCapture()`; without this signal, the
        // parent would SIGKILL the helper before the .mov is finalized.
        // See issue #216.
        let finalizeStart = Date()
        self.finalizeStart = finalizeStart
        let heartbeatTask = Task { [finalizeStart] in
            while !Task.isCancelled {
                let elapsedMs = Int(Date().timeIntervalSince(finalizeStart) * 1000)
                let payload: [String: Any] = [
                    "event": "finalizing",
                    "elapsed_ms": elapsedMs,
                ]
                if let data = try? JSONSerialization.data(withJSONObject: payload),
                   let json = String(data: data, encoding: .utf8) {
                    print(json)
                    fflush(stdout)
                }
                try? await Task.sleep(nanoseconds: 1_000_000_000)
            }
        }

        // Flush and close stem files on the sample queue to serialize
        // with any in-flight writeStemSamples calls. Without this,
        // nil'ing on the main thread races with writes on sampleQueue.
        sampleQueue.sync {
            stemsClosed = true
            voiceStemFile = nil
            systemStemFile = nil
        }

        guard let stream else {
            heartbeatTask.cancel()
            exit(0)
        }

        do {
            try await stream.stopCapture()
        } catch {
            heartbeatTask.cancel()
            fputs("stopCapture failed: \(error)\n", stderr)
            exit(1)
        }

        // Emit elapsed timing on successful stopCapture return. This is the
        // ScreenCaptureKit-side stop time; the .mov finalize (moov atom
        // write) keeps running until recordingOutputDidFinishRecording fires
        // and emits `finalize_complete` with its own elapsed_ms. Pair the two
        // to characterize the duration-to-finalize curve on long captures
        // (#216 / #236).
        let stopReturnedMs = Int(Date().timeIntervalSince(finalizeStart) * 1000)
        let stopPayload: [String: Any] = [
            "event": "stopCapture_returned",
            "elapsed_ms": stopReturnedMs,
        ]
        if let data = try? JSONSerialization.data(withJSONObject: stopPayload),
           let json = String(data: data, encoding: .utf8) {
            print(json)
            fflush(stdout)
        }

        // stopCapture() returns when the framework has been told to stop, but
        // the moov atom may still be in flight: the actual finalize completes
        // when `recordingOutputDidFinishRecording` fires and calls exit(0).
        // Keep the heartbeat alive across that window so the Rust parent
        // doesn't see 30s of silence and SIGKILL us before the .mov is on
        // disk. The heartbeat Task dies naturally when the process exits.
    }

    private func startMonitoring() {
        let timer = DispatchSource.makeTimerSource(queue: sampleQueue)
        timer.schedule(deadline: .now(), repeating: .milliseconds(150))
        timer.setEventHandler { [weak self] in
            guard let self else { return }
            let now = Date()
            let systemLive = self.lastSystemAudioSampleAt.map { now.timeIntervalSince($0) < 1.5 } ?? false
            let micLive = self.lastMicSampleAt.map { now.timeIntervalSince($0) < 1.5 } ?? false
            if !systemLive {
                self.latestSystemLevel = 0
            }
            if !micLive {
                self.latestMicLevel = 0
            }

            let shouldEmit = systemLive || micLive || systemLive != self.lastReportedSystemLive || micLive != self.lastReportedMicLive
            guard shouldEmit else { return }

            self.lastReportedSystemLive = systemLive
            self.lastReportedMicLive = micLive
            let payload: [String: Any] = [
                "event": "health",
                "call_audio_live": systemLive,
                "mic_live": micLive,
                "call_audio_level": self.latestSystemLevel,
                "mic_level": self.latestMicLevel
            ]
            if let data = try? JSONSerialization.data(withJSONObject: payload),
               let json = String(data: data, encoding: .utf8) {
                print(json)
                fflush(stdout)
            }
        }
        timer.resume()
        monitorTimer = timer
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of outputType: SCStreamOutputType) {
        guard CMSampleBufferIsValid(sampleBuffer), CMSampleBufferDataIsReady(sampleBuffer) else {
            return
        }
        let now = Date()
        switch outputType {
        case .audio:
            lastSystemAudioSampleAt = now
            writeStemSamples(sampleBuffer, source: .audio)
        case .microphone:
            lastMicSampleAt = now
            writeStemSamples(sampleBuffer, source: .microphone)
        default:
            break
        }
    }

    private func writeStemSamples(_ sampleBuffer: CMSampleBuffer, source: SCStreamOutputType) {
        // Runs on sampleQueue, the same queue stop() closes the stems on, so
        // this either sees the closed latch or ran entirely before it was set.
        // Dropping these late samples loses at most the few milliseconds
        // between the close and stopCapture() returning; honoring them
        // destroys the whole recording.
        if stemsClosed { return }
        guard let formatDescription = CMSampleBufferGetFormatDescription(sampleBuffer),
              let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(formatDescription)?.pointee else {
            return
        }

        let sourceName: String
        switch source {
        case .microphone:
            sourceName = "microphone"
        case .audio:
            sourceName = "system"
        default:
            return
        }
        reportSourceFormatOnce(sourceName: sourceName, asbd: asbd)

        guard let blockBuffer = CMSampleBufferGetDataBuffer(sampleBuffer) else {
            return
        }

        let sampleCount = CMSampleBufferGetNumSamples(sampleBuffer)
        guard sampleCount > 0 else { return }

        var contiguousLength: Int = 0
        let totalLength = CMBlockBufferGetDataLength(blockBuffer)
        var dataPointer: UnsafeMutablePointer<Int8>?
        let pointerStatus = CMBlockBufferGetDataPointer(
            blockBuffer,
            atOffset: 0,
            lengthAtOffsetOut: &contiguousLength,
            totalLengthOut: nil,
            dataPointerOut: &dataPointer
        )
        guard totalLength > 0 else { return }

        let sampleRate = asbd.mSampleRate

        // Stems are always mono float32 — mix down if multi-channel
        guard let monoFormat = AVAudioFormat(
            commonFormat: .pcmFormatFloat32,
            sampleRate: sampleRate,
            channels: 1,
            interleaved: false
        ) else { return }

        let frameCount = AVAudioFrameCount(sampleCount)
        guard let pcmBuffer = AVAudioPCMBuffer(pcmFormat: monoFormat, frameCapacity: frameCount) else {
            return
        }
        pcmBuffer.frameLength = frameCount

        guard let monoPtr = pcmBuffer.floatChannelData?[0] else { return }
        let decoded: Bool
        if pointerStatus == kCMBlockBufferNoErr,
           let dataPointer,
           contiguousLength >= totalLength {
            decoded = decodeLinearPCMToMono(
                bytes: UnsafeRawBufferPointer(start: dataPointer, count: totalLength),
                frameCount: Int(frameCount),
                asbd: asbd,
                output: monoPtr
            )
        } else {
            var copiedBytes = [UInt8](repeating: 0, count: totalLength)
            let copyStatus: OSStatus = copiedBytes.withUnsafeMutableBytes { destination in
                guard let baseAddress = destination.baseAddress else {
                    return -1
                }
                return CMBlockBufferCopyDataBytes(
                    blockBuffer,
                    atOffset: 0,
                    dataLength: totalLength,
                    destination: baseAddress
                )
            }
            decoded = copyStatus == kCMBlockBufferNoErr && copiedBytes.withUnsafeBytes { bytes in
                decodeLinearPCMToMono(
                    bytes: bytes,
                    frameCount: Int(frameCount),
                    asbd: asbd,
                    output: monoPtr
                )
            }
        }

        guard decoded else {
            reportDecodeFailureOnce(sourceName: sourceName, asbd: asbd)
            return
        }

        // Lazily create the stem only after the first buffer decodes. An
        // unsupported source format must not leave a plausible empty WAV.
        let stemFile: AVAudioFile?
        switch source {
        case .microphone:
            if voiceStemFile == nil, let url = voiceStemURL {
                do {
                    voiceStemFile = try AVAudioFile(forWriting: url, settings: monoFormat.settings)
                } catch {
                    fputs("failed to create voice stem file: \(error)\n", stderr)
                }
            }
            stemFile = voiceStemFile
        case .audio:
            if systemStemFile == nil, let url = systemStemURL {
                do {
                    systemStemFile = try AVAudioFile(forWriting: url, settings: monoFormat.settings)
                } catch {
                    fputs("failed to create system stem file: \(error)\n", stderr)
                }
            }
            stemFile = systemStemFile
        default:
            return
        }

        guard let file = stemFile else { return }

        var sumSquares: Float = 0
        for frame in 0..<Int(frameCount) {
            let sample = monoPtr[frame]
            sumSquares += sample * sample
        }
        let rms = sqrt(sumSquares / max(Float(frameCount), 1))
        let level = UInt32(min(100.0, max(0.0, Double(rms) * 2000.0)))
        switch source {
        case .microphone:
            latestMicLevel = level
        case .audio:
            latestSystemLevel = level
        default:
            break
        }

        do {
            try file.write(from: pcmBuffer)
        } catch {
            fputs("stem write failed: \(error)\n", stderr)
        }
    }

    private func reportSourceFormatOnce(
        sourceName: String,
        asbd: AudioStreamBasicDescription
    ) {
        let signature = sourceFormatSignature(sourceName: sourceName, asbd: asbd)
        guard reportedSourceFormats.insert(signature).inserted else { return }
        let flags = asbd.mFormatFlags
        var payload: [String: Any] = [
            "event": "audio_format",
            "source": sourceName,
            "sample_rate": asbd.mSampleRate,
            "format_id": formatIDString(asbd.mFormatID),
            "format_flags": flags,
            "bits_per_channel": asbd.mBitsPerChannel,
            "bytes_per_packet": asbd.mBytesPerPacket,
            "bytes_per_frame": asbd.mBytesPerFrame,
            "frames_per_packet": asbd.mFramesPerPacket,
            "channels": asbd.mChannelsPerFrame,
            "is_float": (flags & kAudioFormatFlagIsFloat) != 0,
            "is_signed_integer": (flags & kAudioFormatFlagIsSignedInteger) != 0,
            "is_big_endian": (flags & kAudioFormatFlagIsBigEndian) != 0,
            "is_non_interleaved": (flags & kAudioFormatFlagIsNonInterleaved) != 0,
            "is_aligned_high": (flags & kAudioFormatFlagIsAlignedHigh) != 0,
        ]
        if sourceName == "microphone" {
            payload["device_name"] = selectedMicrophoneName ?? ""
            if let selectedMicrophoneDeviceSampleRate {
                payload["device_sample_rate"] = selectedMicrophoneDeviceSampleRate
            }
        }
        queueSourceEvent(payload)
    }

    private func reportDecodeFailureOnce(
        sourceName: String,
        asbd: AudioStreamBasicDescription
    ) {
        let signature = sourceFormatSignature(sourceName: sourceName, asbd: asbd)
        guard reportedDecodeFailures.insert(signature).inserted else { return }
        let payload: [String: Any] = [
            "event": "audio_format_unsupported",
            "source": sourceName,
            "format_id": formatIDString(asbd.mFormatID),
            "format_flags": asbd.mFormatFlags,
            "bits_per_channel": asbd.mBitsPerChannel,
            "bytes_per_packet": asbd.mBytesPerPacket,
            "bytes_per_frame": asbd.mBytesPerFrame,
            "frames_per_packet": asbd.mFramesPerPacket,
            "channels": asbd.mChannelsPerFrame,
        ]
        queueSourceEvent(payload)
        fputs("unsupported \(sourceName) PCM layout; stem buffer withheld\n", stderr)
    }

    private func sourceFormatSignature(
        sourceName: String,
        asbd: AudioStreamBasicDescription
    ) -> String {
        [
            sourceName,
            String(asbd.mFormatID),
            String(asbd.mFormatFlags),
            String(asbd.mSampleRate),
            String(asbd.mBitsPerChannel),
            String(asbd.mBytesPerPacket),
            String(asbd.mBytesPerFrame),
            String(asbd.mFramesPerPacket),
            String(asbd.mChannelsPerFrame),
        ].joined(separator: ":")
    }

    private func queueSourceEvent(_ payload: [String: Any]) {
        if protocolReady {
            emitJSON(payload)
        } else {
            pendingSourceEvents.append(payload)
        }
    }

    func recordingOutputDidStartRecording(_ recordingOutput: SCRecordingOutput) {
        print("ready")
        fflush(stdout)

        // Never emit device-selection status before `ready`: Rust treats the
        // first stdout line as a strict readiness handshake.
        if let microphoneSelectionEvent,
           let data = try? JSONSerialization.data(withJSONObject: microphoneSelectionEvent),
           let json = String(data: data, encoding: .utf8) {
            print(json)
            fflush(stdout)
        }

        // Report stem paths so the Rust side knows where to find them
        let stemInfo: [String: Any] = [
            "event": "stems",
            "voice_stem": voiceStemURL?.path ?? "",
            "system_stem": systemStemURL?.path ?? ""
        ]
        if let data = try? JSONSerialization.data(withJSONObject: stemInfo),
           let json = String(data: data, encoding: .utf8) {
            print(json)
            fflush(stdout)
        }

        sampleQueue.async { [weak self] in
            guard let self else { return }
            self.protocolReady = true
            self.pendingSourceEvents.forEach(emitJSON)
            self.pendingSourceEvents.removeAll()
        }
    }

    func recordingOutputDidFinishRecording(_ recordingOutput: SCRecordingOutput) {
        // Emit elapsed time from the start of stop() to actual .mov finalize.
        // Pair with `stopCapture_returned` to size the moov-write tail (#216
        // / #236).
        if let start = finalizeStart {
            let elapsedMs = Int(Date().timeIntervalSince(start) * 1000)
            let payload: [String: Any] = [
                "event": "finalize_complete",
                "elapsed_ms": elapsedMs,
            ]
            if let data = try? JSONSerialization.data(withJSONObject: payload),
               let json = String(data: data, encoding: .utf8) {
                print(json)
                fflush(stdout)
            }
        }
        exit(0)
    }

    func recordingOutput(
        _ recordingOutput: SCRecordingOutput,
        didFailWithError error: Error
    ) {
        fputs("recordingOutput failed: \(error)\n", stderr)
        exit(1)
    }
}

@main
struct NativeCallRecordMain {
    // Keep the signal source alive after `run()` returns so the SIGTERM handler
    // remains installed for the lifetime of the helper.
    nonisolated(unsafe) static var retainedStopSource: DispatchSourceSignal?

    static func main() {
        Task {
            await run()
        }
        dispatchMain()
    }

    static func run() async {
        if CommandLine.arguments.count == 2,
           CommandLine.arguments[1] == "--self-test-pcm-decoder" {
            exit(runPCMDecoderSelfTest() ? 0 : 1)
        }

        guard #available(macOS 15.0, *) else {
            fputs("ScreenCaptureKit recording output requires macOS 15.0 or newer.\n", stderr)
            exit(1)
        }

        if CommandLine.arguments.count == 2,
           CommandLine.arguments[1] == "--list-microphones" {
            let payload: [String: Any] = [
                "devices": availableMicrophones().map(\.localizedName)
            ]
            do {
                let data = try JSONSerialization.data(withJSONObject: payload)
                FileHandle.standardOutput.write(data)
                FileHandle.standardOutput.write(Data("\n".utf8))
                exit(0)
            } catch {
                fputs("failed to serialize microphone inventory: \(error)\n", stderr)
                exit(1)
            }
        }

        guard CommandLine.arguments.count >= 2 else {
            fputs("usage: system_audio_record <output.mov> [--microphone-name <exact name>]\n", stderr)
            exit(1)
        }

        let outputURL = URL(fileURLWithPath: CommandLine.arguments[1])
        var requestedMicrophoneName: String?
        if let flagIndex = CommandLine.arguments.firstIndex(of: "--microphone-name"),
           CommandLine.arguments.indices.contains(flagIndex + 1) {
            requestedMicrophoneName = CommandLine.arguments[flagIndex + 1]
        }
        let recorder = NativeCallRecorder(
            outputURL: outputURL,
            requestedMicrophoneName: requestedMicrophoneName
        )

        signal(SIGTERM, SIG_IGN)
        let stopSource = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
        stopSource.setEventHandler {
            Task {
                await recorder.stop()
            }
        }
        stopSource.resume()
        NativeCallRecordMain.retainedStopSource = stopSource

        do {
            try await recorder.start()
        } catch {
            fputs("start failed: \(error)\n", stderr)
            exit(1)
        }
    }
}
