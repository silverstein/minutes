import AVFoundation
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

@available(macOS 15.0, *)
final class NativeCallRecorder: NSObject, SCRecordingOutputDelegate, SCStreamOutput {
    private var stream: SCStream?
    private var recordingOutput: SCRecordingOutput?
    private let outputURL: URL
    private let requestedMicrophoneName: String?
    private var selectedMicrophoneID: String?
    private var selectedMicrophoneName: String?
    private var microphoneSelectionEvent: [String: Any]?
    private let sampleQueue = DispatchQueue(label: "minutes.system-audio.samples")
    private var monitorTimer: DispatchSourceTimer?
    private var lastSystemAudioSampleAt: Date?
    private var lastMicSampleAt: Date?
    private var lastReportedSystemLive = false
    private var lastReportedMicLive = false
    private var latestSystemLevel: UInt32 = 0
    private var latestMicLevel: UInt32 = 0

    // Per-source stem writers
    private var voiceStemFile: AVAudioFile?
    private var systemStemFile: AVAudioFile?
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
        guard let formatDescription = CMSampleBufferGetFormatDescription(sampleBuffer),
              let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(formatDescription)?.pointee else {
            return
        }

        guard let blockBuffer = CMSampleBufferGetDataBuffer(sampleBuffer) else {
            return
        }

        let sampleCount = CMSampleBufferGetNumSamples(sampleBuffer)
        guard sampleCount > 0 else { return }

        var lengthAtOffset: Int = 0
        var totalLength: Int = 0
        var dataPointer: UnsafeMutablePointer<Int8>?
        let status = CMBlockBufferGetDataPointer(blockBuffer, atOffset: 0, lengthAtOffsetOut: &lengthAtOffset, totalLengthOut: &totalLength, dataPointerOut: &dataPointer)
        guard status == kCMBlockBufferNoErr, let data = dataPointer else {
            return
        }

        let channelCount = Int(asbd.mChannelsPerFrame)
        let sampleRate = asbd.mSampleRate
        let isFloat = (asbd.mFormatFlags & kAudioFormatFlagIsFloat) != 0

        // Stems are always mono float32 — mix down if multi-channel
        guard let monoFormat = AVAudioFormat(
            commonFormat: .pcmFormatFloat32,
            sampleRate: sampleRate,
            channels: 1,
            interleaved: false
        ) else { return }

        // Lazily create the stem file on first samples
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

        // Mix multi-channel source data to mono float32.
        // ScreenCaptureKit may deliver interleaved or non-interleaved audio.
        let isNonInterleaved = (asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved) != 0
        let frameCount = AVAudioFrameCount(sampleCount)
        guard let pcmBuffer = AVAudioPCMBuffer(pcmFormat: monoFormat, frameCapacity: frameCount) else {
            return
        }
        pcmBuffer.frameLength = frameCount

        guard let monoPtr = pcmBuffer.floatChannelData?[0] else { return }
        let bytesPerSample = isFloat ? 4 : 2

        if isNonInterleaved {
            // Non-interleaved: each channel is a separate plane of `frameCount` samples.
            // The CMBlockBuffer contains them sequentially: [ch0 frames][ch1 frames]...
            let planeSize = Int(frameCount) * bytesPerSample
            for frame in 0..<Int(frameCount) {
                var sum: Float = 0.0
                for ch in 0..<channelCount {
                    let offset = ch * planeSize + frame * bytesPerSample
                    guard offset + bytesPerSample <= totalLength else { break }
                    if isFloat {
                        var val: Float = 0.0
                        memcpy(&val, data.advanced(by: offset), 4)
                        sum += val
                    } else {
                        var val: Int16 = 0
                        memcpy(&val, data.advanced(by: offset), 2)
                        sum += Float(val) / 32768.0
                    }
                }
                monoPtr[frame] = sum / Float(channelCount)
            }
        } else {
            // Interleaved: samples are [ch0 ch1 ch0 ch1 ...]
            for frame in 0..<Int(frameCount) {
                var sum: Float = 0.0
                for ch in 0..<channelCount {
                    let offset = (frame * channelCount + ch) * bytesPerSample
                    guard offset + bytesPerSample <= totalLength else { break }
                    if isFloat {
                        var val: Float = 0.0
                        memcpy(&val, data.advanced(by: offset), 4)
                        sum += val
                    } else {
                        var val: Int16 = 0
                        memcpy(&val, data.advanced(by: offset), 2)
                        sum += Float(val) / 32768.0
                    }
                }
                monoPtr[frame] = sum / Float(channelCount)
            }
        }

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
