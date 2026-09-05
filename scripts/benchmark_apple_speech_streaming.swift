@preconcurrency import AVFAudio
import CoreMedia
import Foundation
import Speech

struct Event: Codable {
    let elapsedMs: UInt64
    let audioMs: Int
    let rangeStartMs: Int
    let rangeEndMs: Int
    let finalizationMs: Int
    let isFinal: Bool
    let text: String
}

actor EventLog {
    private var events: [Event] = []
    private let verbose: Bool

    init(verbose: Bool = true) {
        self.verbose = verbose
    }

    func append(_ event: Event) {
        events.append(event)
        if verbose {
            fputs("event elapsed=\(event.elapsedMs)ms audio=\(event.audioMs)ms final=\(event.isFinal) text=\(event.text)\n", stderr)
        }
    }

    func snapshot() -> [Event] { events }
}

enum BenchmarkError: Error, CustomStringConvertible {
    case failed(String)

    var description: String {
        switch self {
        case .failed(let message): return message
        }
    }
}

struct EvaluationExpectations: Decodable {
    let referenceText: String
    let requiredTerms: [String]
    let forbiddenTerms: [String]

    enum CodingKeys: String, CodingKey {
        case referenceText
        case requiredTerms
        case forbiddenTerms
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        referenceText = try values.decode(String.self, forKey: .referenceText)
        requiredTerms = try values.decodeIfPresent([String].self, forKey: .requiredTerms) ?? []
        forbiddenTerms = try values.decodeIfPresent([String].self, forKey: .forbiddenTerms) ?? []
    }
}

func normalizedWords(_ text: String) -> [String] {
    text
        .lowercased()
        .split(whereSeparator: \Character.isWhitespace)
        .map { word in
            String(word.filter { $0.isLetter || $0.isNumber || $0 == "'" })
        }
        .filter { !$0.isEmpty }
}

func wordErrorRate(reference: String, hypothesis: String) -> Double {
    let expected = normalizedWords(reference)
    let actual = normalizedWords(hypothesis)
    var previous = Array(0...actual.count)
    for (row, expectedWord) in expected.enumerated() {
        var current = [row + 1]
        for (column, actualWord) in actual.enumerated() {
            if expectedWord == actualWord {
                current.append(previous[column])
            } else {
                current.append(1 + min(previous[column], previous[column + 1], current[column]))
            }
        }
        previous = current
    }
    return Double(previous[actual.count]) / Double(max(expected.count, 1))
}

func finalizedTranscript(from events: [Event]) -> String {
    var segments: [Event] = []
    for event in events where event.isFinal {
        segments.removeAll { earlier in
            earlier.rangeStartMs < event.rangeEndMs && event.rangeStartMs < earlier.rangeEndMs
        }
        segments.append(event)
    }
    return segments
        .sorted { ($0.rangeStartMs, $0.rangeEndMs) < ($1.rangeStartMs, $1.rangeEndMs) }
        .map(\.text)
        .joined(separator: " ")
        .trimmingCharacters(in: .whitespacesAndNewlines)
}

@available(macOS 26.0, *)
func readAndConvert(_ path: String, modules: [any SpeechModule]) async throws -> AVAudioPCMBuffer {
    let file = try AVAudioFile(forReading: URL(fileURLWithPath: path))
    guard let source = AVAudioPCMBuffer(
        pcmFormat: file.processingFormat,
        frameCapacity: AVAudioFrameCount(file.length)
    ) else {
        throw BenchmarkError.failed("could not allocate source audio")
    }
    try file.read(into: source)
    guard let targetFormat = await SpeechAnalyzer.bestAvailableAudioFormat(
        compatibleWith: modules,
        considering: source.format
    ) else {
        throw BenchmarkError.failed("no compatible audio format")
    }
    if source.format == targetFormat {
        return source
    }
    guard let converter = AVAudioConverter(from: source.format, to: targetFormat) else {
        throw BenchmarkError.failed("could not create audio converter")
    }
    let ratio = targetFormat.sampleRate / source.format.sampleRate
    let capacity = AVAudioFrameCount((Double(source.frameLength) * ratio).rounded(.up)) + 1
    guard let converted = AVAudioPCMBuffer(pcmFormat: targetFormat, frameCapacity: capacity) else {
        throw BenchmarkError.failed("could not allocate converted audio")
    }
    var supplied = false
    var conversionError: NSError?
    let status = converter.convert(to: converted, error: &conversionError) { _, outputStatus in
        if supplied {
            outputStatus.pointee = .endOfStream
            return nil
        }
        supplied = true
        outputStatus.pointee = .haveData
        return source
    }
    if status == .error {
        throw conversionError ?? BenchmarkError.failed("audio conversion failed")
    }
    return converted
}

func copyFrames(
    from source: AVAudioPCMBuffer,
    startFrame: AVAudioFramePosition,
    frameCount: AVAudioFrameCount
) throws -> AVAudioPCMBuffer {
    guard let chunk = AVAudioPCMBuffer(pcmFormat: source.format, frameCapacity: frameCount) else {
        throw BenchmarkError.failed("could not allocate audio chunk")
    }
    chunk.frameLength = frameCount
    let bytesPerFrame = Int(source.format.streamDescription.pointee.mBytesPerFrame)
    let buffers = UnsafeMutableAudioBufferListPointer(source.mutableAudioBufferList)
    let destinationBuffers = UnsafeMutableAudioBufferListPointer(chunk.mutableAudioBufferList)
    for index in 0..<buffers.count {
        guard let sourceData = buffers[index].mData, let destinationData = destinationBuffers[index].mData else {
            throw BenchmarkError.failed("audio buffer has no data")
        }
        memcpy(
            destinationData,
            sourceData.advanced(by: Int(startFrame) * bytesPerFrame),
            Int(frameCount) * bytesPerFrame
        )
        destinationBuffers[index].mDataByteSize = UInt32(Int(frameCount) * bytesPerFrame)
    }
    return chunk
}

@available(macOS 26.0, *)
func pacedInputs(_ audio: AVAudioPCMBuffer, chunkMs: Int) -> AsyncThrowingStream<AnalyzerInput, Error> {
    AsyncThrowingStream { continuation in
        Task {
            do {
                let framesPerChunk = max(
                    AVAudioFrameCount(audio.format.sampleRate * Double(chunkMs) / 1_000.0),
                    1
                )
                let timescale = max(Int32(audio.format.sampleRate.rounded()), 1)
                let started = DispatchTime.now().uptimeNanoseconds
                var frame: AVAudioFramePosition = 0
                while frame < AVAudioFramePosition(audio.frameLength) {
                    let remaining = AVAudioFramePosition(audio.frameLength) - frame
                    let count = min(framesPerChunk, AVAudioFrameCount(remaining))
                    let chunk = try copyFrames(from: audio, startFrame: frame, frameCount: count)
                    continuation.yield(
                        AnalyzerInput(
                            buffer: chunk,
                            bufferStartTime: CMTime(value: CMTimeValue(frame), timescale: timescale)
                        )
                    )
                    frame += AVAudioFramePosition(count)
                    if frame < AVAudioFramePosition(audio.frameLength) {
                        let targetElapsed = UInt64(
                            Double(frame) / audio.format.sampleRate * 1_000_000_000
                        )
                        let elapsed = DispatchTime.now().uptimeNanoseconds - started
                        if targetElapsed > elapsed {
                            try await Task.sleep(for: .nanoseconds(targetElapsed - elapsed))
                        }
                    }
                }
                continuation.finish()
            } catch {
                continuation.finish(throwing: error)
            }
        }
    }
}

@available(macOS 26.0, *)
func printSummary(
    audio: AVAudioPCMBuffer,
    chunkMs: Int,
    presetName: String,
    events: [Event],
    expectations: EvaluationExpectations?,
    metricsOnly: Bool
) throws {
    let useful = events.filter { !$0.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    let provisional = useful.filter { !$0.isFinal }
    let finalized = useful.filter(\.isFinal)
    let usefulCadences = zip(useful.dropFirst(), useful).map { Int($0.0.elapsedMs - $0.1.elapsedMs) }
    let provisionalCadences = zip(provisional.dropFirst(), provisional).map {
        Int($0.0.elapsedMs - $0.1.elapsedMs)
    }
    var revisionCount = 0
    for (index, event) in provisional.enumerated() {
        let revisedEarlierRange = provisional[..<index].contains { earlier in
            let overlaps = earlier.rangeStartMs < event.rangeEndMs && event.rangeStartMs < earlier.rangeEndMs
            return overlaps && earlier.text != event.text
        }
        if revisedEarlierRange {
            revisionCount += 1
        }
    }
    let durationMs = Int(Double(audio.frameLength) / audio.format.sampleRate * 1_000)
    let completionLagMs = useful.last.map { max(0, Int($0.elapsedMs) - durationMs) }
    let finalText = finalizedTranscript(from: useful)
    var summary: [String: Any] = [
        "audioDurationMs": durationMs,
        "chunkMs": chunkMs,
        "preset": presetName,
        "eventCount": events.count,
        "provisionalEventCount": provisional.count,
        "finalEventCount": finalized.count,
        "revisionCount": revisionCount,
        "firstUsefulMs": useful.first?.elapsedMs as Any,
        "firstProvisionalMs": provisional.first?.elapsedMs as Any,
        "maxUsefulCadenceMs": usefulCadences.max() as Any,
        "maxProvisionalCadenceMs": provisionalCadences.max() as Any,
        "completionLagMs": completionLagMs as Any,
    ]
    if let expectations {
        let lowercasedFinal = finalText.lowercased()
        summary["referenceWordCount"] = normalizedWords(expectations.referenceText).count
        summary["punctuationInsensitiveWer"] = wordErrorRate(
            reference: expectations.referenceText,
            hypothesis: finalText
        )
        summary["requiredTermsMissingCount"] = expectations.requiredTerms.filter {
            !lowercasedFinal.contains($0.lowercased())
        }.count
        summary["forbiddenTermsFoundCount"] = expectations.forbiddenTerms.filter {
            lowercasedFinal.contains($0.lowercased())
        }.count
    }
    if !metricsOnly {
        summary["finalTranscript"] = finalText
        summary["events"] = try JSONSerialization.jsonObject(with: JSONEncoder().encode(events))
    }
    let json = try JSONSerialization.data(withJSONObject: summary, options: [.prettyPrinted, .sortedKeys])
    print(String(decoding: json, as: UTF8.self))
}

@available(macOS 26.0, *)
func runSpeech(
    audioPath: String,
    chunkMs: Int,
    presetName: String,
    expectations: EvaluationExpectations?,
    metricsOnly: Bool
) async throws {
    let locale = Locale(identifier: "en-US")
    let transcriber: SpeechTranscriber
    switch presetName {
    case "speech-fast-final":
        transcriber = SpeechTranscriber(
            locale: locale,
            transcriptionOptions: [],
            reportingOptions: [.fastResults],
            attributeOptions: [.audioTimeRange]
        )
    case "speech-progressive", "speech":
        // Apple's progressive preset is the live-audio configuration. It
        // combines fast results with volatile (replaceable) results. The old
        // benchmark used fastResults alone, which measured a fast final result
        // but did not exercise the drafts Minutes would consume while speech
        // is still in progress.
        transcriber = SpeechTranscriber(
            locale: locale,
            preset: .timeIndexedProgressiveTranscription
        )
    default:
        throw BenchmarkError.failed("unknown SpeechTranscriber preset: \(presetName)")
    }
    let modules: [any SpeechModule] = [transcriber]
    if let request = try await AssetInventory.assetInstallationRequest(supporting: modules) {
        try await request.downloadAndInstall()
    }
    let audio = try await readAndConvert(audioPath, modules: modules)
    let analyzer = SpeechAnalyzer(modules: modules)
    let started = DispatchTime.now().uptimeNanoseconds
    let log = EventLog(verbose: !metricsOnly)
    let resultTask = Task {
        for try await result in transcriber.results {
            let elapsed = (DispatchTime.now().uptimeNanoseconds - started) / 1_000_000
            let text = String(result.text.characters).trimmingCharacters(in: .whitespacesAndNewlines)
            if !text.isEmpty {
                await log.append(Event(
                    elapsedMs: elapsed,
                    audioMs: Int(CMTimeGetSeconds(CMTimeRangeGetEnd(result.range)) * 1_000),
                    rangeStartMs: Int(CMTimeGetSeconds(result.range.start) * 1_000),
                    rangeEndMs: Int(CMTimeGetSeconds(CMTimeRangeGetEnd(result.range)) * 1_000),
                    finalizationMs: Int(CMTimeGetSeconds(result.resultsFinalizationTime) * 1_000),
                    isFinal: result.isFinal,
                    text: text
                ))
            }
        }
    }
    if let lastSample = try await analyzer.analyzeSequence(pacedInputs(audio, chunkMs: chunkMs)) {
        try await analyzer.finalizeAndFinish(through: lastSample)
    } else {
        await analyzer.cancelAndFinishNow()
    }
    try await resultTask.value
    try printSummary(
        audio: audio,
        chunkMs: chunkMs,
        presetName: presetName,
        events: await log.snapshot(),
        expectations: expectations,
        metricsOnly: metricsOnly
    )
}

@available(macOS 26.0, *)
func run() async throws {
    guard CommandLine.arguments.count >= 2 else {
        throw BenchmarkError.failed("usage: benchmark_apple_speech_streaming AUDIO [chunk-ms] [preset|speech]")
    }
    let audioPath = CommandLine.arguments[1]
    let chunkMs = CommandLine.arguments.count > 2 ? Int(CommandLine.arguments[2]) ?? 100 : 100
    let locale = Locale(identifier: "en-US")
    let presetName = CommandLine.arguments.count > 3 ? CommandLine.arguments[3] : "progressive-short"
    let expectations = CommandLine.arguments.count > 4 && CommandLine.arguments[4] != "-"
        ? try JSONDecoder().decode(
            EvaluationExpectations.self,
            from: Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[4]))
        )
        : nil
    let metricsOnly = CommandLine.arguments.dropFirst(5).contains("--metrics-only")
    if presetName.hasPrefix("speech") {
        try await runSpeech(
            audioPath: audioPath,
            chunkMs: chunkMs,
            presetName: presetName,
            expectations: expectations,
            metricsOnly: metricsOnly
        )
        return
    }
    let transcriber: DictationTranscriber
    if presetName == "volatile-no-punctuation" {
        transcriber = DictationTranscriber(
            locale: locale,
            contentHints: [.shortForm],
            transcriptionOptions: [],
            reportingOptions: [.volatileResults, .frequentFinalization],
            attributeOptions: [.audioTimeRange]
        )
    } else {
        let preset: DictationTranscriber.Preset
        switch presetName {
        case "phrase": preset = .phrase
        case "short": preset = .shortDictation
        case "progressive-long": preset = .progressiveLongDictation
        case "long": preset = .longDictation
        default: preset = .progressiveShortDictation
        }
        transcriber = DictationTranscriber(locale: locale, preset: preset)
    }
    let modules: [any SpeechModule] = [transcriber]
    if let request = try await AssetInventory.assetInstallationRequest(supporting: modules) {
        try await request.downloadAndInstall()
    }
    let audio = try await readAndConvert(audioPath, modules: modules)
    let analyzer = SpeechAnalyzer(modules: modules)
    let started = DispatchTime.now().uptimeNanoseconds
    let log = EventLog(verbose: !metricsOnly)
    let resultTask = Task {
        for try await result in transcriber.results {
            let elapsed = (DispatchTime.now().uptimeNanoseconds - started) / 1_000_000
            let text = String(result.text.characters).trimmingCharacters(in: .whitespacesAndNewlines)
            if !text.isEmpty {
                await log.append(Event(
                    elapsedMs: elapsed,
                    audioMs: Int(CMTimeGetSeconds(CMTimeRangeGetEnd(result.range)) * 1_000),
                    rangeStartMs: Int(CMTimeGetSeconds(result.range.start) * 1_000),
                    rangeEndMs: Int(CMTimeGetSeconds(CMTimeRangeGetEnd(result.range)) * 1_000),
                    finalizationMs: Int(CMTimeGetSeconds(result.resultsFinalizationTime) * 1_000),
                    isFinal: result.isFinal,
                    text: text
                ))
            }
        }
    }
    if let lastSample = try await analyzer.analyzeSequence(pacedInputs(audio, chunkMs: chunkMs)) {
        try await analyzer.finalizeAndFinish(through: lastSample)
    } else {
        await analyzer.cancelAndFinishNow()
    }
    try await resultTask.value
    let events = await log.snapshot()
    try printSummary(
        audio: audio,
        chunkMs: chunkMs,
        presetName: presetName,
        events: events,
        expectations: expectations,
        metricsOnly: metricsOnly
    )
}

@main
struct Main {
    static func main() async {
        guard #available(macOS 26.0, *) else {
            fputs("macOS 26 or newer is required\n", stderr)
            exit(2)
        }
        do {
            try await run()
        } catch {
            fputs("benchmark failed: \(error)\n", stderr)
            exit(1)
        }
    }
}
