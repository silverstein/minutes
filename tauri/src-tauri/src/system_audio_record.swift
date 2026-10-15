import AVFoundation
import AudioToolbox
import CoreMedia
import CoreFoundation
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

        let midpoint = pow(2.0, Double(bitsPerChannel - 1))
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

private func monoFloatBuffer(samples: [Float], sampleRate: Double) -> AVAudioPCMBuffer? {
    guard let format = AVAudioFormat(
        commonFormat: .pcmFormatFloat32,
        sampleRate: sampleRate,
        channels: 1,
        interleaved: false
    ),
    let buffer = AVAudioPCMBuffer(
        pcmFormat: format,
        frameCapacity: AVAudioFrameCount(samples.count)
    ),
    let destination = buffer.floatChannelData?[0] else {
        return nil
    }
    buffer.frameLength = AVAudioFrameCount(samples.count)
    destination.update(from: samples, count: samples.count)
    return buffer
}

private func convertMonoBuffer(
    _ sourceBuffer: AVAudioPCMBuffer,
    to targetFormat: AVAudioFormat,
    using converter: AVAudioConverter
) throws -> AVAudioPCMBuffer {
    let ratio = targetFormat.sampleRate / sourceBuffer.format.sampleRate
    let capacity =
        AVAudioFrameCount((Double(sourceBuffer.frameLength) * ratio).rounded(.up)) + 1
    guard let converted = AVAudioPCMBuffer(
        pcmFormat: targetFormat,
        frameCapacity: max(capacity, 1)
    ) else {
        throw NSError(
            domain: "MinutesSystemAudioRecord",
            code: 3,
            userInfo: [NSLocalizedDescriptionKey: "could not allocate converted audio buffer"]
        )
    }

    var providedInput = false
    var conversionError: NSError?
    let status = converter.convert(to: converted, error: &conversionError) { _, outStatus in
        if providedInput {
            outStatus.pointee = .endOfStream
            return nil
        }
        providedInput = true
        outStatus.pointee = .haveData
        return sourceBuffer
    }
    if let conversionError {
        throw conversionError
    }
    if status == .error {
        throw NSError(
            domain: "MinutesSystemAudioRecord",
            code: 4,
            userInfo: [NSLocalizedDescriptionKey: "AVAudioConverter failed"]
        )
    }
    return converted
}

private final class FixedFormatStemWriter {
    private let url: URL
    private var file: AVAudioFile?
    private var converter: AVAudioConverter?
    private var converterInputFormat: AVAudioFormat?
    private(set) var framesSilenced: UInt64 = 0
    private(set) var framesWritten: UInt64 = 0

    /// One converter per (input format → stem format). Recreating it per buffer
    /// discards the resampler's filter state at every buffer boundary and
    /// allocates ~50 times a second; reusing it keeps the resampled stream
    /// continuous across a rate transition.
    private func converter(
        from inputFormat: AVAudioFormat,
        to targetFormat: AVAudioFormat
    ) throws -> AVAudioConverter {
        if let converter, let converterInputFormat, converterInputFormat == inputFormat {
            return converter
        }
        guard let created = AVAudioConverter(from: inputFormat, to: targetFormat) else {
            throw NSError(
                domain: "MinutesSystemAudioRecord",
                code: 2,
                userInfo: [NSLocalizedDescriptionKey: "could not create AVAudioConverter"]
            )
        }
        converter = created
        converterInputFormat = inputFormat
        return created
    }

    init(url: URL) {
        self.url = url
    }

    @discardableResult
    func write(_ buffer: AVAudioPCMBuffer) throws -> AVAudioFrameCount {
        if file == nil {
            file = try AVAudioFile(forWriting: url, settings: buffer.format.settings)
        }
        guard let file else { return 0 }

        let output: AVAudioPCMBuffer
        if buffer.format.sampleRate == file.processingFormat.sampleRate {
            output = buffer
        } else {
            output = try convertMonoBuffer(
                buffer,
                to: file.processingFormat,
                using: try converter(from: buffer.format, to: file.processingFormat)
            )
        }
        try file.write(from: output)
        framesWritten += UInt64(output.frameLength)
        return output.frameLength
    }

    @discardableResult
    func writeSilence(
        sourceFrameCount: AVAudioFrameCount,
        sourceSampleRate: Double
    ) throws -> AVAudioFrameCount {
        guard let file, sourceSampleRate > 0 else { return 0 }
        let ratio = file.processingFormat.sampleRate / sourceSampleRate
        let frameCount = max(
            AVAudioFrameCount((Double(sourceFrameCount) * ratio).rounded()),
            1
        )
        guard let silence = AVAudioPCMBuffer(
            pcmFormat: file.processingFormat,
            frameCapacity: frameCount
        ),
        let samples = silence.floatChannelData?[0] else {
            throw NSError(
                domain: "MinutesSystemAudioRecord",
                code: 5,
                userInfo: [NSLocalizedDescriptionKey: "could not allocate silence buffer"]
            )
        }
        silence.frameLength = frameCount
        samples.initialize(repeating: 0, count: Int(frameCount))
        try file.write(from: silence)
        framesSilenced += UInt64(frameCount)
        framesWritten += UInt64(frameCount)
        return frameCount
    }

    func close() {
        file = nil
        converter = nil
        converterInputFormat = nil
    }
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

    var unsignedSilenceBytes: [UInt8] = []
    appendLittleEndian(UInt16(0x8000), to: &unsignedSilenceBytes)
    let unsignedSilence = decodedSamples(
        bytes: unsignedSilenceBytes,
        frameCount: 1,
        asbd: testASBD(
            channels: 1,
            bitsPerChannel: 16,
            bytesPerFrame: 2,
            flags: kAudioFormatFlagIsPacked
        )
    )

    let tolerance: Float = 0.000_01
    let cases = [int16, highAligned, int32, stereoFloat]
    let decoderCasesPassed = cases.allSatisfy { samples in
        guard let samples, samples.count == 2 else { return false }
        return abs(samples[0] - 0.5) < tolerance && abs(samples[1] + 0.5) < tolerance
    }
    let unsignedSilencePassed = unsignedSilence.map {
        $0.count == 1 && abs($0[0]) < tolerance
    } ?? false

    var stemCasesPassed = false
    var selfTestFramesSilenced: UInt64 = 0
    let temporaryDirectory = FileManager.default.temporaryDirectory
        .appendingPathComponent("minutes-pcm-self-test-\(UUID().uuidString)")
    do {
        try FileManager.default.createDirectory(
            at: temporaryDirectory,
            withIntermediateDirectories: true
        )
        defer { try? FileManager.default.removeItem(at: temporaryDirectory) }

        let rateChangeURL = temporaryDirectory.appendingPathComponent("rate-change.wav")
        let rateChangeWriter = FixedFormatStemWriter(url: rateChangeURL)
        guard let firstSegment = monoFloatBuffer(
            samples: [Float](repeating: 0.25, count: 480),
            sampleRate: 48_000
        ) else { throw NSError(domain: "MinutesPCMTest", code: 1) }
        try rateChangeWriter.write(firstSegment)

        var secondSegmentBytes: [UInt8] = []
        for _ in 0..<160 {
            appendLittleEndian(Int32(16_384) << 16, to: &secondSegmentBytes)
        }
        guard let secondSamples = decodedSamples(
            bytes: secondSegmentBytes,
            frameCount: 160,
            asbd: testASBD(
                channels: 1,
                bitsPerChannel: 16,
                bytesPerFrame: 4,
                flags: signedHighAligned
            )
        ),
        let secondSegment = monoFloatBuffer(samples: secondSamples, sampleRate: 16_000) else {
            throw NSError(domain: "MinutesPCMTest", code: 2)
        }
        try rateChangeWriter.write(secondSegment)
        rateChangeWriter.close()

        let rateChangeFile = try AVAudioFile(forReading: rateChangeURL)
        guard let rateChangeAudio = AVAudioPCMBuffer(
            pcmFormat: rateChangeFile.processingFormat,
            frameCapacity: AVAudioFrameCount(rateChangeFile.length)
        ) else { throw NSError(domain: "MinutesPCMTest", code: 3) }
        try rateChangeFile.read(into: rateChangeAudio)
        let rateChangeLengthPassed = rateChangeFile.fileFormat.sampleRate == 48_000
            && rateChangeAudio.frameLength >= 900
        let tailStart = Int(rateChangeAudio.frameLength) * 3 / 4
        let tailPresent = rateChangeAudio.floatChannelData.map { channels in
            guard Int(rateChangeAudio.frameLength) > tailStart else { return false }
            let tail = UnsafeBufferPointer(
                start: channels[0] + tailStart,
                count: Int(rateChangeAudio.frameLength) - tailStart
            )
            return tail.contains { abs($0) > 0.1 }
        } ?? false

        let silenceURL = temporaryDirectory.appendingPathComponent("silence-fill.wav")
        let silenceWriter = FixedFormatStemWriter(url: silenceURL)
        guard let supported = monoFloatBuffer(
            samples: [Float](repeating: 0.25, count: 120),
            sampleRate: 48_000
        ) else { throw NSError(domain: "MinutesPCMTest", code: 4) }
        try silenceWriter.write(supported)
        let silencedFrames = try silenceWriter.writeSilence(
            sourceFrameCount: 80,
            sourceSampleRate: 48_000
        )
        silenceWriter.close()

        let silenceFile = try AVAudioFile(forReading: silenceURL)
        guard let silenceAudio = AVAudioPCMBuffer(
            pcmFormat: silenceFile.processingFormat,
            frameCapacity: AVAudioFrameCount(silenceFile.length)
        ) else { throw NSError(domain: "MinutesPCMTest", code: 5) }
        try silenceFile.read(into: silenceAudio)
        let silenceSpanPassed = silenceAudio.frameLength == 200
            && silencedFrames == 80
            && silenceWriter.framesSilenced == 80
            && silenceWriter.framesWritten == 200
            && (silenceAudio.floatChannelData.map { channels in
                (120..<200).allSatisfy { abs(channels[0][$0]) < tolerance }
            } ?? false)

        stemCasesPassed = rateChangeLengthPassed && tailPresent && silenceSpanPassed
        selfTestFramesSilenced = silenceWriter.framesSilenced
    } catch {
        fputs("PCM stem self-test failed: \(error)\n", stderr)
    }

    let passed = decoderCasesPassed && unsignedSilencePassed && stemCasesPassed
    if passed {
        emitJSON([
            "event": "pcm_decoder_self_test",
            "status": "ok",
            "cases": cases.count + 3,
            "frames_silenced": selfTestFramesSilenced,
        ])
    } else {
        fputs("PCM decoder self-test failed\n", stderr)
    }
    return passed
}

// The recording output may finish after an involuntary stream stop. A finished
// file is not evidence that the user requested the end of capture.
private final class CaptureLifecycle {
    private let lock = NSLock()
    private var stopRequested = false
    private var interrupted = false

    func requestStop() {
        lock.lock()
        defer { lock.unlock() }
        stopRequested = true
    }

    func recordInterruption() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard !stopRequested, !interrupted else { return false }
        interrupted = true
        return true
    }

    func completionExitStatus() -> Int32 {
        lock.lock()
        defer { lock.unlock() }
        return stopRequested && !interrupted ? 0 : 1
    }
}

private func runCaptureLifecycleSelfTest() -> Bool {
    let unexpected = CaptureLifecycle()
    let requested = CaptureLifecycle()
    requested.requestStop()
    let interrupted = CaptureLifecycle()
    let firstInterruption = interrupted.recordInterruption()
    let duplicateInterruption = interrupted.recordInterruption()
    interrupted.requestStop()
    let passed = unexpected.completionExitStatus() == 1
        && requested.completionExitStatus() == 0
        && !requested.recordInterruption()
        && firstInterruption && !duplicateInterruption
        && interrupted.completionExitStatus() == 1
        && Thread.isMainThread
    emitJSON([
        "event": "capture_lifecycle_self_test",
        "status": passed ? "ok" : "failed",
        "main_thread": Thread.isMainThread,
        "cases": 5,
    ])
    return passed
}

@available(macOS 15.0, *)
final class NativeCallRecorder: NSObject, SCRecordingOutputDelegate, SCStreamOutput, SCStreamDelegate {
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
    private var lastReportedSourceFormats: [String: String] = [:]
    private var reportedDecodeFailures = Set<String>()
    private var reportedStemWriteFailures = Set<String>()
    private let captureLifecycle = CaptureLifecycle()

    // Per-source stem writers
    private var voiceStemWriter: FixedFormatStemWriter?
    private var systemStemWriter: FixedFormatStemWriter?
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

        let stream = SCStream(filter: filter, configuration: configuration, delegate: self)
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
        if let voiceStemURL {
            voiceStemWriter = FixedFormatStemWriter(url: voiceStemURL)
        }
        if let systemStemURL {
            systemStemWriter = FixedFormatStemWriter(url: systemStemURL)
        }

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
        captureLifecycle.requestStop()
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
            voiceStemWriter?.close()
            systemStemWriter?.close()
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

            // Emit even when both inputs stop delivering buffers. The parent
            // distinguishes a healthy timer from actual successful stem writes.

            self.lastReportedSystemLive = systemLive
            self.lastReportedMicLive = micLive
            let payload: [String: Any] = [
                "event": "health",
                "call_audio_live": systemLive,
                "mic_live": micLive,
                "call_audio_level": self.latestSystemLevel,
                "mic_level": self.latestMicLevel,
                "mic_frames_written": self.voiceStemWriter?.framesWritten ?? 0,
                "system_frames_written": self.systemStemWriter?.framesWritten ?? 0,
                "mic_frames_silenced": self.voiceStemWriter?.framesSilenced ?? 0,
                "system_frames_silenced": self.systemStemWriter?.framesSilenced ?? 0,
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
        reportSourceFormatTransition(sourceName: sourceName, asbd: asbd)

        let sampleCount = CMSampleBufferGetNumSamples(sampleBuffer)
        guard sampleCount > 0 else { return }
        let frameCount = AVAudioFrameCount(sampleCount)
        let sampleRate = asbd.mSampleRate

        guard let blockBuffer = CMSampleBufferGetDataBuffer(sampleBuffer) else {
            silenceUndecodableFrames(
                source: source,
                sourceName: sourceName,
                frameCount: frameCount,
                sampleRate: sampleRate,
                asbd: asbd
            )
            return
        }

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
        guard totalLength > 0 else {
            silenceUndecodableFrames(
                source: source,
                sourceName: sourceName,
                frameCount: frameCount,
                sampleRate: sampleRate,
                asbd: asbd
            )
            return
        }

        // Stems are always mono float32 — mix down if multi-channel
        guard let monoFormat = AVAudioFormat(
            commonFormat: .pcmFormatFloat32,
            sampleRate: sampleRate,
            channels: 1,
            interleaved: false
        ) else {
            silenceUndecodableFrames(
                source: source,
                sourceName: sourceName,
                frameCount: frameCount,
                sampleRate: sampleRate,
                asbd: asbd
            )
            return
        }

        guard let pcmBuffer = AVAudioPCMBuffer(pcmFormat: monoFormat, frameCapacity: frameCount) else {
            silenceUndecodableFrames(
                source: source,
                sourceName: sourceName,
                frameCount: frameCount,
                sampleRate: sampleRate,
                asbd: asbd
            )
            return
        }
        pcmBuffer.frameLength = frameCount

        guard let monoPtr = pcmBuffer.floatChannelData?[0] else {
            silenceUndecodableFrames(
                source: source,
                sourceName: sourceName,
                frameCount: frameCount,
                sampleRate: sampleRate,
                asbd: asbd
            )
            return
        }
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
            silenceUndecodableFrames(
                source: source,
                sourceName: sourceName,
                frameCount: frameCount,
                sampleRate: sampleRate,
                asbd: asbd
            )
            return
        }

        // Lazily create the stem only after the first buffer decodes. The
        // writer then retains that file's processing format for the capture.
        let stemWriter: FixedFormatStemWriter?
        switch source {
        case .microphone:
            stemWriter = voiceStemWriter
        case .audio:
            stemWriter = systemStemWriter
        default:
            return
        }

        guard let stemWriter else { return }

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
            try stemWriter.write(pcmBuffer)
        } catch {
            reportStemWriteFailureOnce(sourceName: sourceName, error: error)
        }
    }

    private func silenceUndecodableFrames(
        source: SCStreamOutputType,
        sourceName: String,
        frameCount: AVAudioFrameCount,
        sampleRate: Double,
        asbd: AudioStreamBasicDescription
    ) {
        reportDecodeFailureOnce(sourceName: sourceName, asbd: asbd)
        let writer: FixedFormatStemWriter?
        switch source {
        case .microphone:
            writer = voiceStemWriter
        case .audio:
            writer = systemStemWriter
        default:
            return
        }
        do {
            try writer?.writeSilence(
                sourceFrameCount: frameCount,
                sourceSampleRate: sampleRate
            )
        } catch {
            reportStemWriteFailureOnce(sourceName: sourceName, error: error)
        }
    }

    private func reportSourceFormatTransition(
        sourceName: String,
        asbd: AudioStreamBasicDescription
    ) {
        let signature = sourceFormatSignature(sourceName: sourceName, asbd: asbd)
        guard lastReportedSourceFormats[sourceName] != signature else { return }
        lastReportedSourceFormats[sourceName] = signature
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
            "sample_rate": asbd.mSampleRate,
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

    private func reportStemWriteFailureOnce(sourceName: String, error: Error) {
        let description = String(describing: error)
        let signature = "\(sourceName):\(description)"
        guard reportedStemWriteFailures.insert(signature).inserted else { return }
        fputs("stem write failed for \(sourceName): \(description)\n", stderr)
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

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        guard captureLifecycle.recordInterruption() else { return }
        let nativeError = error as NSError
        // Do not emit localized descriptions or userInfo: they can include
        // unrelated application/window information. Domain and code suffice.
        emitJSON([
            "event": "capture_interrupted",
            "reason": "stream_error",
            "error_domain": nativeError.domain,
            "error_code": nativeError.code,
            "observed_at": ISO8601DateFormatter().string(from: Date()),
        ])
    }

    func recordingOutputDidFinishRecording(_ recordingOutput: SCRecordingOutput) {
        if captureLifecycle.recordInterruption() {
            emitJSON([
                "event": "capture_interrupted",
                "reason": "unexpected_output_finish",
                "observed_at": ISO8601DateFormatter().string(from: Date()),
            ])
        }
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
        exit(captureLifecycle.completionExitStatus())
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
        // ScreenCaptureKit is a WindowServer client. Keep the main CFRunLoop
        // servicing its Mach/event sources, not just the main dispatch queue.
        // dispatchMain parks the original main thread and can leave the client
        // unable to answer WindowServer responsiveness pings.
        CFRunLoopRun()
    }

    static func run() async {
        if CommandLine.arguments.count == 2,
           CommandLine.arguments[1] == "--self-test-capture-lifecycle" {
            // Exercise the same main run loop without opening any capture.
            DispatchQueue.main.async {
                exit(runCaptureLifecycleSelfTest() ? 0 : 1)
            }
            return
        }
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
