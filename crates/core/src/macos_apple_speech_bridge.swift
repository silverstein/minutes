import AVFAudio
import CoreMedia
import Darwin
import Foundation
import Speech

private let bridgeSchemaVersion = 1

private struct BridgeTranscriptSegment: Codable {
    let startMs: UInt64
    let durationMs: UInt64
    let text: String
}

private struct BridgeTranscriptionResponse: Codable {
    let kind: String
    let schemaVersion: Int
    let moduleId: String
    let locale: String
    let ensureAssets: Bool
    let osVersion: String
    let runtimeSupported: Bool
    let assetStatusBefore: String
    let assetStatusAfter: String
    let totalElapsedMs: UInt64
    let firstResultElapsedMs: UInt64?
    let transcript: String
    let wordCount: Int
    let segments: [BridgeTranscriptSegment]
    let notes: [String]
    let error: String?
}

private enum BridgeError: Error {
    case invalidInput(String)
    case unsupportedRuntime(String)
    case speechFailure(String)
}

private final class ResponseBox: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Data?

    func install(_ data: Data) {
        lock.lock()
        value = data
        lock.unlock()
    }

    func take() -> Data? {
        lock.lock()
        defer { lock.unlock() }
        return value
    }
}

@_cdecl("minutes_apple_speech_transcribe_pcm")
public func minutesAppleSpeechTranscribePCM(
    _ samples: UnsafePointer<Float>?,
    _ sampleCount: Int,
    _ modePointer: UnsafePointer<CChar>?,
    _ localePointer: UnsafePointer<CChar>?,
    _ ensureAssetsValue: Int32,
    _ responseLength: UnsafeMutablePointer<Int>?
) -> UnsafeMutablePointer<UInt8>? {
    guard let samples,
          sampleCount > 0,
          let modePointer,
          let localePointer,
          let responseLength else {
        return nil
    }
    let mode = String(cString: modePointer)
    let locale = String(cString: localePointer)
    guard mode == "speech" || mode == "dictation",
          !locale.isEmpty,
          locale.utf8.count <= 256,
          sampleCount <= 16_000 * 10 * 60 else {
        return nil
    }

    let copiedSamples = Array(UnsafeBufferPointer(start: samples, count: sampleCount))
    guard copiedSamples.allSatisfy(\.isFinite) else {
        return nil
    }

    // Answer the capability question here, on the calling thread, before any
    // Speech-typed work is scheduled. Signed run 30660755527 trapped inside
    // swift_getTypeByMangledName on the XPC handler thread itself, below this
    // function and above Task.detached, so a guard inside the async body ran
    // too late to prevent it. Returning the failure response now keeps the
    // helper alive and lets the parent fall back to Whisper.
    if !speechRuntimeSymbolsResolvable() {
        return encodeResponse(
            failureResponse(
                mode: mode,
                localeIdentifier: locale,
                ensureAssets: ensureAssetsValue == 1,
                error: BridgeError.unsupportedRuntime(
                    "SpeechAnalyzer types are unavailable on this device."
                )
            ),
            responseLength
        )
    }

    guard let data = runSpeechTranscription(
        copiedSamples,
        mode: mode,
        locale: locale,
        ensureAssets: ensureAssetsValue == 1
    ) else {
        return nil
    }
    return copyOutResponse(data, responseLength)
}

/// Run the Speech-typed work behind a boundary the optimizer will not cross.
///
/// The macOS 26 Speech type metadata is materialized where it is used, and at
/// `-O` those accessors can be hoisted to the entry of whatever function
/// contains them. Keeping this out of the `@_cdecl` entry means an incapable
/// device returns from the capability guard without ever reaching a site that
/// resolves a Speech symbolic reference.
@inline(never)
private func runSpeechTranscription(
    _ copiedSamples: [Float],
    mode: String,
    locale: String,
    ensureAssets: Bool
) -> Data? {
    let ensureAssetsValue: Int32 = ensureAssets ? 1 : 0
    let responseBox = ResponseBox()
    let semaphore = DispatchSemaphore(value: 0)
    Task.detached {
        let response: BridgeTranscriptionResponse
        do {
            response = try await transcribePrivatePCM(
                copiedSamples,
                mode: mode,
                localeIdentifier: locale,
                ensureAssets: ensureAssetsValue == 1
            )
        } catch {
            response = failureResponse(
                mode: mode,
                localeIdentifier: locale,
                ensureAssets: ensureAssetsValue == 1,
                error: error
            )
        }
        responseBox.install((try? JSONEncoder().encode(response)) ?? Data())
        semaphore.signal()
    }
    semaphore.wait()
    guard let data = responseBox.take(), !data.isEmpty else {
        return nil
    }
    return data
}

/// Encode a response and hand it to the caller using the same ownership
/// contract as the normal path, so the early capability return frees through
/// `minutes_apple_speech_free_response` exactly like every other response.
private func encodeResponse(
    _ response: BridgeTranscriptionResponse,
    _ responseLength: UnsafeMutablePointer<Int>
) -> UnsafeMutablePointer<UInt8>? {
    guard let data = try? JSONEncoder().encode(response), !data.isEmpty else {
        return nil
    }
    return copyOutResponse(data, responseLength)
}

private func copyOutResponse(
    _ data: Data,
    _ responseLength: UnsafeMutablePointer<Int>
) -> UnsafeMutablePointer<UInt8> {
    let response = UnsafeMutablePointer<UInt8>.allocate(capacity: data.count)
    _ = data.copyBytes(to: UnsafeMutableBufferPointer(start: response, count: data.count))
    responseLength.pointee = data.count
    return response
}

@_cdecl("minutes_apple_speech_free_response")
public func minutesAppleSpeechFreeResponse(
    _ response: UnsafeMutablePointer<UInt8>?,
    _ responseLength: Int
) {
    guard let response, responseLength > 0 else {
        return
    }
    response.initialize(repeating: 0, count: responseLength)
    response.deallocate()
}

private func failureResponse(
    mode: String,
    localeIdentifier: String,
    ensureAssets: Bool,
    error: Error
) -> BridgeTranscriptionResponse {
    BridgeTranscriptionResponse(
        kind: "transcription",
        schemaVersion: bridgeSchemaVersion,
        moduleId: mode == "dictation" ? "dictation-transcriber" : "speech-transcriber",
        locale: localeIdentifier,
        ensureAssets: ensureAssets,
        osVersion: ProcessInfo.processInfo.operatingSystemVersionString,
        runtimeSupported: false,
        assetStatusBefore: "unknown",
        assetStatusAfter: "unknown",
        totalElapsedMs: 0,
        firstResultElapsedMs: nil,
        transcript: "",
        wordCount: 0,
        segments: [],
        notes: [],
        error: String(describing: error)
    )
}

/// Mangled Swift symbols the macOS 26 Speech modules must export before any
/// Speech-typed expression is evaluated.
///
/// The bridge is compiled against a macOS 11 deployment target, so every
/// macOS 26 Speech symbol is a weak import. `dyld_info -fixups` shows these as
/// `bind Speech/... [weak-import]`, which resolve to null on a device whose
/// Speech framework does not export them. `#available` only tests the OS
/// version, so on such a device it passes and the first Speech-typed
/// expression traps inside `swift_getTypeByMangledName` with an untrappable
/// `swift::fatalError` rather than throwing.
///
/// Signed acceptance run 30647233333 hit exactly that on a `VirtualMac2,1`
/// runner reporting Apple Intelligence `deviceNotCapable`, while the same code
/// path completes on real Apple Silicon at both macOS 26.5 and 26.6. Probing
/// the symbols keeps that device on the Whisper fallback instead of aborting
/// the helper.
private let speechRuntimeSymbols = [
    // Symbolic references in mangled type names resolve to *descriptors*, not
    // to metadata accessors, so probe the descriptors themselves. The protocol
    // descriptor matters most: every `[any SpeechModule]` existential in this
    // bridge resolves through it.
    "$s6Speech0A6ModuleMp",
    "$s6Speech0A11TranscriberCMn",
    "$s6Speech0A11TranscriberC6ResultVMn",
    "$s6Speech0A11TranscriberC15ReportingOptionOMn",
    "$s6Speech0A11TranscriberC21ResultAttributeOptionOMn",
    "$s6Speech0A8AnalyzerC7OptionsVMn",
    "$s6Speech13AnalyzerInputVMn",
    "$s6Speech20DictationTranscriberC11ContentHintVMn",
    "$s6Speech0A11TranscriberCAA0A6ModuleAAMc",
]

private func speechRuntimeSymbolsResolvable() -> Bool {
    // RTLD_DEFAULT: search every globally visible image.
    let handle = UnsafeMutableRawPointer(bitPattern: -2)
    return speechRuntimeSymbols.allSatisfy { dlsym(handle, $0) != nil }
}

private func transcribePrivatePCM(
    _ samples: [Float],
    mode: String,
    localeIdentifier: String,
    ensureAssets: Bool
) async throws -> BridgeTranscriptionResponse {
    guard #available(macOS 26.0, *) else {
        throw BridgeError.unsupportedRuntime(
            "SpeechAnalyzer APIs require macOS 26.0 or newer at runtime."
        )
    }
    guard speechRuntimeSymbolsResolvable() else {
        throw BridgeError.unsupportedRuntime(
            "SpeechAnalyzer types are unavailable on this device."
        )
    }
    guard let sourceFormat = AVAudioFormat(
        standardFormatWithSampleRate: 16_000,
        channels: 1
    ),
    let sourceBuffer = AVAudioPCMBuffer(
        pcmFormat: sourceFormat,
        frameCapacity: AVAudioFrameCount(samples.count)
    ),
    let channel = sourceBuffer.floatChannelData?.pointee else {
        throw BridgeError.speechFailure("Failed to allocate private PCM buffer.")
    }
    sourceBuffer.frameLength = AVAudioFrameCount(samples.count)
    samples.withUnsafeBufferPointer { source in
        channel.update(from: source.baseAddress!, count: samples.count)
    }

    if mode == "speech" {
        return try await transcribeSpeech(
            sourceBuffer,
            localeIdentifier: localeIdentifier,
            ensureAssets: ensureAssets
        )
    }
    if mode == "dictation" {
        return try await transcribeDictation(
            sourceBuffer,
            localeIdentifier: localeIdentifier,
            ensureAssets: ensureAssets
        )
    }
    throw BridgeError.invalidInput("Unknown Apple Speech mode.")
}

@available(macOS 26.0, *)
private func transcribeSpeech(
    _ sourceBuffer: AVAudioPCMBuffer,
    localeIdentifier: String,
    ensureAssets: Bool
) async throws -> BridgeTranscriptionResponse {
    let locale = Locale(identifier: localeIdentifier)
    let transcriber = SpeechTranscriber(
        locale: locale,
        transcriptionOptions: [],
        reportingOptions: [.fastResults],
        attributeOptions: [.audioTimeRange]
    )
    let statusBefore = assetStatusString(await AssetInventory.status(forModules: [transcriber]))
    if ensureAssets {
        try await ensureAssetsInstalled(for: [transcriber])
    }
    let statusAfter = assetStatusString(await AssetInventory.status(forModules: [transcriber]))
    guard SpeechTranscriber.isAvailable else {
        throw BridgeError.speechFailure("SpeechTranscriber unavailable on this device.")
    }
    let analyzer = SpeechAnalyzer(modules: [transcriber])
    let input = try await analyzerInput(from: sourceBuffer, modules: [transcriber])
    let started = DispatchTime.now().uptimeNanoseconds
    let state = BridgeResultState()
    let resultsTask = Task<[BridgeTranscriptSegment], Error> {
        var segments: [BridgeTranscriptSegment] = []
        for try await result in transcriber.results {
            let elapsed = (DispatchTime.now().uptimeNanoseconds - started) / 1_000_000
            await state.recordFirst(elapsed)
            let text = String(result.text.characters)
                .trimmingCharacters(in: .whitespacesAndNewlines)
            if !text.isEmpty {
                segments.append(
                    BridgeTranscriptSegment(
                        startMs: timeRangeStartMs(result.range),
                        durationMs: timeRangeDurationMs(result.range),
                        text: text
                    )
                )
            }
        }
        await state.recordFinished(
            (DispatchTime.now().uptimeNanoseconds - started) / 1_000_000
        )
        return segments
    }
    if let lastSample = try await analyzer.analyzeSequence(singleInputStream(input)) {
        try await analyzer.finalizeAndFinish(through: lastSample)
    } else {
        await analyzer.cancelAndFinishNow()
    }
    return await makeResponse(
        moduleId: "speech-transcriber",
        localeIdentifier: localeIdentifier,
        ensureAssets: ensureAssets,
        statusBefore: statusBefore,
        statusAfter: statusAfter,
        state: state,
        segments: try await resultsTask.value
    )
}

@available(macOS 26.0, *)
private func transcribeDictation(
    _ sourceBuffer: AVAudioPCMBuffer,
    localeIdentifier: String,
    ensureAssets: Bool
) async throws -> BridgeTranscriptionResponse {
    let locale = Locale(identifier: localeIdentifier)
    let transcriber = DictationTranscriber(
        locale: locale,
        contentHints: [.shortForm],
        transcriptionOptions: [],
        reportingOptions: [.frequentFinalization],
        attributeOptions: [.audioTimeRange]
    )
    let statusBefore = assetStatusString(await AssetInventory.status(forModules: [transcriber]))
    if ensureAssets {
        try await ensureAssetsInstalled(for: [transcriber])
    }
    let statusAfter = assetStatusString(await AssetInventory.status(forModules: [transcriber]))
    if statusAfter == "unsupported" {
        throw BridgeError.speechFailure(
            "DictationTranscriber unsupported for this locale or device."
        )
    }
    let analyzer = SpeechAnalyzer(modules: [transcriber])
    let input = try await analyzerInput(from: sourceBuffer, modules: [transcriber])
    let started = DispatchTime.now().uptimeNanoseconds
    let state = BridgeResultState()
    let resultsTask = Task<[BridgeTranscriptSegment], Error> {
        var segments: [BridgeTranscriptSegment] = []
        for try await result in transcriber.results {
            let elapsed = (DispatchTime.now().uptimeNanoseconds - started) / 1_000_000
            await state.recordFirst(elapsed)
            let text = String(result.text.characters)
                .trimmingCharacters(in: .whitespacesAndNewlines)
            if !text.isEmpty {
                segments.append(
                    BridgeTranscriptSegment(
                        startMs: timeRangeStartMs(result.range),
                        durationMs: timeRangeDurationMs(result.range),
                        text: text
                    )
                )
            }
        }
        await state.recordFinished(
            (DispatchTime.now().uptimeNanoseconds - started) / 1_000_000
        )
        return segments
    }
    if let lastSample = try await analyzer.analyzeSequence(singleInputStream(input)) {
        try await analyzer.finalizeAndFinish(through: lastSample)
    } else {
        await analyzer.cancelAndFinishNow()
    }
    return await makeResponse(
        moduleId: "dictation-transcriber",
        localeIdentifier: localeIdentifier,
        ensureAssets: ensureAssets,
        statusBefore: statusBefore,
        statusAfter: statusAfter,
        state: state,
        segments: try await resultsTask.value
    )
}

private actor BridgeResultState {
    private var firstResultElapsedMs: UInt64?
    private var finishedElapsedMs: UInt64 = 0

    func recordFirst(_ elapsed: UInt64) {
        if firstResultElapsedMs == nil {
            firstResultElapsedMs = elapsed
        }
    }

    func recordFinished(_ elapsed: UInt64) {
        finishedElapsedMs = elapsed
    }

    func snapshot() -> (UInt64?, UInt64) {
        (firstResultElapsedMs, finishedElapsedMs)
    }
}

@available(macOS 26.0, *)
private func analyzerInput(
    from sourceBuffer: AVAudioPCMBuffer,
    modules: [any SpeechModule]
) async throws -> AnalyzerInput {
    guard let targetFormat = await SpeechAnalyzer.bestAvailableAudioFormat(
        compatibleWith: modules,
        considering: sourceBuffer.format
    ) else {
        throw BridgeError.speechFailure(
            "No compatible audio format is available for the selected modules."
        )
    }
    let workingBuffer = if audioFormatsMatch(sourceBuffer.format, targetFormat) {
        sourceBuffer
    } else {
        try convertBuffer(sourceBuffer, to: targetFormat)
    }
    return AnalyzerInput(
        buffer: workingBuffer,
        bufferStartTime: CMTime(
            value: 0,
            timescale: max(Int32(targetFormat.sampleRate.rounded()), 1)
        )
    )
}

@available(macOS 26.0, *)
private func singleInputStream(_ input: AnalyzerInput) -> AsyncStream<AnalyzerInput> {
    AsyncStream { continuation in
        continuation.yield(input)
        continuation.finish()
    }
}

private func convertBuffer(
    _ sourceBuffer: AVAudioPCMBuffer,
    to targetFormat: AVAudioFormat
) throws -> AVAudioPCMBuffer {
    guard let converter = AVAudioConverter(from: sourceBuffer.format, to: targetFormat) else {
        throw BridgeError.speechFailure("Failed to create Apple Speech audio converter.")
    }
    let ratio = targetFormat.sampleRate / sourceBuffer.format.sampleRate
    let capacity =
        AVAudioFrameCount((Double(sourceBuffer.frameLength) * ratio).rounded(.up)) + 1
    guard let target = AVAudioPCMBuffer(
        pcmFormat: targetFormat,
        frameCapacity: max(capacity, 1)
    ) else {
        throw BridgeError.speechFailure("Failed to allocate converted audio buffer.")
    }
    var provided = false
    var conversionError: NSError?
    let status = converter.convert(to: target, error: &conversionError) { _, outStatus in
        if provided {
            outStatus.pointee = .endOfStream
            return nil
        }
        provided = true
        outStatus.pointee = .haveData
        return sourceBuffer
    }
    if status == .error {
        throw conversionError
            ?? BridgeError.speechFailure("Apple Speech audio conversion failed.")
    }
    return target
}

@available(macOS 26.0, *)
private func ensureAssetsInstalled(for modules: [any SpeechModule]) async throws {
    let status = await AssetInventory.status(forModules: modules)
    if case .installed = status {
        return
    }
    guard let request = try await AssetInventory.assetInstallationRequest(
        supporting: modules
    ) else {
        throw BridgeError.speechFailure(
            "No Apple Speech asset installation request was available."
        )
    }
    try await request.downloadAndInstall()
}

@available(macOS 26.0, *)
private func assetStatusString(_ status: AssetInventory.Status) -> String {
    switch status {
    case .installed:
        return "installed"
    case .supported:
        return "supported"
    case .unsupported:
        return "unsupported"
    case .downloading:
        return "downloading"
    @unknown default:
        return "unknown"
    }
}

private func audioFormatsMatch(_ lhs: AVAudioFormat, _ rhs: AVAudioFormat) -> Bool {
    lhs.sampleRate == rhs.sampleRate
        && lhs.channelCount == rhs.channelCount
        && lhs.commonFormat == rhs.commonFormat
        && lhs.isInterleaved == rhs.isInterleaved
}

private func timeRangeStartMs(_ range: CMTimeRange) -> UInt64 {
    guard range.start.isNumeric else { return 0 }
    return UInt64(max(CMTimeGetSeconds(range.start) * 1_000, 0).rounded())
}

private func timeRangeDurationMs(_ range: CMTimeRange) -> UInt64 {
    guard range.duration.isNumeric else { return 0 }
    return UInt64(max(CMTimeGetSeconds(range.duration) * 1_000, 0).rounded())
}

private func makeResponse(
    moduleId: String,
    localeIdentifier: String,
    ensureAssets: Bool,
    statusBefore: String,
    statusAfter: String,
    state: BridgeResultState,
    segments: [BridgeTranscriptSegment]
) async -> BridgeTranscriptionResponse {
    let timing = await state.snapshot()
    let transcript = segments.map(\.text).joined(separator: " ")
    let empty = transcript.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    return BridgeTranscriptionResponse(
        kind: "transcription",
        schemaVersion: bridgeSchemaVersion,
        moduleId: moduleId,
        locale: localeIdentifier,
        ensureAssets: ensureAssets,
        osVersion: ProcessInfo.processInfo.operatingSystemVersionString,
        runtimeSupported: true,
        assetStatusBefore: statusBefore,
        assetStatusAfter: statusAfter,
        totalElapsedMs: timing.1,
        firstResultElapsedMs: timing.0,
        transcript: transcript,
        wordCount: transcript.split(whereSeparator: \.isWhitespace).count,
        segments: segments,
        notes: empty ? ["Analyzer completed without emitting transcription results."] : [],
        error: empty ? "No transcription results emitted" : nil
    )
}
