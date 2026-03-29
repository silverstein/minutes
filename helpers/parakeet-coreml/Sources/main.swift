import Darwin
import FluidAudio
import Foundation

private let defaultModelDirectory = "~/.minutes/models/parakeet-coreml/"
private let samplesPerSecond: Double = 16_000
private let minimumTranscribableSamples = 16_000
private let pauseSplitThreshold: TimeInterval = 0.75
private let maxSegmentCharacters = 120
private let posixLocale = Locale(identifier: "en_US_POSIX")

private struct CLIError: LocalizedError {
    let message: String

    var errorDescription: String? { message }
}

private enum CLIControl: Error {
    case help(String)
}

private struct BatchSegment {
    let start: TimeInterval
    let end: TimeInterval
    let text: String
}

private struct ResolvedModelDirectory {
    let directory: URL
    let version: AsrModelVersion
}

private enum LanguagePreference {
    case automatic
    case english
    case other(String)
}

private enum Mode {
    case batch(audioPath: String)
    case stream
}

private struct CLIOptions {
    let mode: Mode
    let modelDirectory: String
    let language: String?
}

private final class Transcriber {
    private let manager: AsrManager

    private init(manager: AsrManager) {
        self.manager = manager
    }

    static func load(modelDirectory: String, language: String?) async throws -> Transcriber {
        let resolved = try resolveModelDirectory(basePath: modelDirectory, language: language)
        let models = try await AsrModels.load(from: resolved.directory, version: resolved.version)
        let manager = AsrManager(config: .default)
        try await manager.initialize(models: models)
        return Transcriber(manager: manager)
    }

    func transcribeFile(at url: URL) async throws -> ASRResult {
        try await manager.transcribe(url, source: .system)
    }

    func transcribeSamples(_ samples: [Float]) async throws -> ASRResult {
        try await manager.transcribe(samples, source: .microphone)
    }

    private static func resolveModelDirectory(basePath: String, language: String?) throws -> ResolvedModelDirectory {
        let baseURL = expandedURL(for: basePath)
        let preference = languagePreference(from: language)
        let versions = preferredVersions(for: preference)
        let v2Folder = AsrModels.defaultCacheDirectory(for: .v2).lastPathComponent
        let v3Folder = AsrModels.defaultCacheDirectory(for: .v3).lastPathComponent

        var triedPaths: [String] = []

        for version in versions {
            let folderName: String
            switch version {
            case .v2:
                folderName = v2Folder
            case .v3:
                folderName = v3Folder
            }
            for candidate in candidateDirectories(baseURL: baseURL, folderName: folderName) {
                triedPaths.append(candidate.path)
                if AsrModels.modelsExist(at: candidate, version: version) {
                    return ResolvedModelDirectory(directory: candidate, version: version)
                }
            }
        }

        let languageHelp: String
        switch preference {
        case .automatic:
            languageHelp = "No compatible Parakeet CoreML model directory was found."
        case .english:
            languageHelp = "No English-capable Parakeet CoreML model directory was found."
        case .other(let language):
            languageHelp = "No multilingual Parakeet CoreML model directory was found for language '\(language)'."
        }

        throw CLIError(
            message: """
            \(languageHelp)
            Checked:
            \(triedPaths.map { "  - \($0)" }.joined(separator: "\n"))
            """
        )
    }

    private static func candidateDirectories(baseURL: URL, folderName: String) -> [URL] {
        let candidates = [
            baseURL,
            baseURL.appendingPathComponent(folderName, isDirectory: true),
            baseURL.deletingLastPathComponent().appendingPathComponent(folderName, isDirectory: true),
        ]

        var unique: [URL] = []
        var seen = Set<String>()

        for candidate in candidates {
            let standardized = candidate.standardizedFileURL
            if seen.insert(standardized.path).inserted {
                unique.append(standardized)
            }
        }

        return unique
    }

    private static func preferredVersions(for preference: LanguagePreference) -> [AsrModelVersion] {
        switch preference {
        case .automatic:
            return [.v3, .v2]
        case .english:
            return [.v2, .v3]
        case .other:
            return [.v3]
        }
    }

    private static func languagePreference(from language: String?) -> LanguagePreference {
        guard let language else {
            return .automatic
        }

        let trimmed = language.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            return .automatic
        }

        let normalized = trimmed.lowercased()
        if normalized == "auto" {
            return .automatic
        }

        let englishHints = ["en", "en-us", "en-gb", "english"]
        if englishHints.contains(normalized) {
            return .english
        }

        // TODO: Verify whether future FluidAudio releases expose explicit per-request language selection.
        return .other(trimmed)
    }

    private static func expandedURL(for path: String) -> URL {
        URL(fileURLWithPath: (path as NSString).expandingTildeInPath).standardizedFileURL
    }
}

private enum BatchFormatter {
    static func lines(for result: ASRResult) -> [String] {
        segments(for: result).map { segment in
            "[\(formatSeconds(segment.start)) - \(formatSeconds(segment.end))] \(segment.text)"
        }
    }

    private static func segments(for result: ASRResult) -> [BatchSegment] {
        if let tokenTimings = result.tokenTimings, !tokenTimings.isEmpty {
            let fromTimings = segments(from: tokenTimings)
            if !fromTimings.isEmpty {
                return fromTimings
            }
        }

        let text = normalizedText(result.text)
        guard !text.isEmpty else {
            return []
        }

        return [
            BatchSegment(
                start: 0,
                end: max(result.duration, 0),
                text: text
            )
        ]
    }

    private static func segments(from tokenTimings: [TokenTiming]) -> [BatchSegment] {
        var segments: [BatchSegment] = []
        var currentStart: TimeInterval?
        var currentEnd: TimeInterval = 0
        var currentText = ""

        func flushCurrentSegment() {
            let text = normalizedText(currentText)
            guard let start = currentStart, !text.isEmpty else {
                currentStart = nil
                currentEnd = 0
                currentText = ""
                return
            }

            segments.append(BatchSegment(start: start, end: currentEnd, text: text))
            currentStart = nil
            currentEnd = 0
            currentText = ""
        }

        for index in tokenTimings.indices {
            let tokenTiming = tokenTimings[index]
            let tokenText = tokenTiming.token

            guard !normalizedText(tokenText).isEmpty else {
                continue
            }

            if currentStart == nil {
                currentStart = tokenTiming.startTime
            }

            currentText.append(tokenText)
            currentEnd = tokenTiming.endTime

            let isLast = index == tokenTimings.endIndex - 1
            let gapToNext = isLast ? 0 : tokenTimings[index + 1].startTime - tokenTiming.endTime
            let shouldSplit =
                tokenEndsSentence(tokenText) ||
                gapToNext > pauseSplitThreshold ||
                normalizedText(currentText).count >= maxSegmentCharacters

            if shouldSplit {
                flushCurrentSegment()
            }
        }

        flushCurrentSegment()
        return segments
    }

    private static func tokenEndsSentence(_ token: String) -> Bool {
        guard let lastCharacter = normalizedText(token).last else {
            return false
        }

        return ".?!".contains(lastCharacter)
    }

    private static func normalizedText(_ text: String) -> String {
        text
            .replacingOccurrences(of: "\\s+", with: " ", options: .regularExpression)
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private static func formatSeconds(_ seconds: TimeInterval) -> String {
        String(format: "%.2f", locale: posixLocale, seconds)
    }
}

private final class StreamSession {
    private let transcriber: Transcriber
    private var samples: [Float] = []

    init(transcriber: Transcriber) {
        self.transcriber = transcriber
    }

    func run() async -> Int32 {
        while let line = readLine(strippingNewline: true) {
            if line.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                continue
            }

            do {
                let shouldQuit = try await handle(line: line)
                if shouldQuit {
                    return 0
                }
            } catch {
                emitStreamError(error.localizedDescription, logToStandardError: true)
            }
        }

        return 0
    }

    private func handle(line: String) async throws -> Bool {
        guard let data = line.data(using: .utf8) else {
            throw CLIError(message: "Input line was not valid UTF-8.")
        }

        let json = try JSONSerialization.jsonObject(with: data)
        guard let object = json as? [String: Any] else {
            throw CLIError(message: "Input line must be a JSON object.")
        }

        guard let command = object["cmd"] as? String else {
            throw CLIError(message: "Input object is missing a string 'cmd' field.")
        }

        switch command {
        case "audio":
            try appendSamples(from: object)
        case "transcribe":
            try await emitTranscription(type: "partial")
        case "finalize":
            try await emitTranscription(type: "final")
        case "reset":
            samples.removeAll(keepingCapacity: true)
        case "quit":
            return true
        default:
            throw CLIError(message: "Unknown command '\(command)'.")
        }

        return false
    }

    private func appendSamples(from object: [String: Any]) throws {
        guard let rawSamples = object["samples"] as? [Any] else {
            throw CLIError(message: "The 'audio' command requires a numeric 'samples' array.")
        }

        // Validate the whole payload before mutating the session buffer so malformed
        // chunks do not partially leak into subsequent transcriptions.
        var parsedSamples: [Float] = []
        parsedSamples.reserveCapacity(rawSamples.count)
        for value in rawSamples {
            guard let number = value as? NSNumber else {
                throw CLIError(message: "The 'samples' array must only contain numbers.")
            }
            if CFGetTypeID(number as CFTypeRef) == CFBooleanGetTypeID() {
                throw CLIError(message: "The 'samples' array must only contain numbers.")
            }
            parsedSamples.append(number.floatValue)
        }

        samples.append(contentsOf: parsedSamples)
    }

    private func emitTranscription(type: String) async throws {
        let duration = Double(samples.count) / samplesPerSecond
        guard samples.count >= minimumTranscribableSamples else {
            writeJSONObject(
                [
                    "type": type,
                    "text": "",
                    "duration_secs": duration,
                ]
            )
            return
        }

        let result = try await transcriber.transcribeSamples(samples)
        writeJSONObject(
            [
                "type": type,
                "text": result.text,
                "duration_secs": result.duration,
            ]
        )
    }

    private func emitStreamError(_ message: String, logToStandardError: Bool) {
        if logToStandardError {
            writeStandardError(message)
        }

        writeJSONObject(
            [
                "type": "error",
                "message": message,
            ]
        )
    }
}

@main
struct ParakeetCoreMLCLI {
    static func main() async {
        let exitCode = await run()
        Darwin.exit(exitCode)
    }

    private static func run() async -> Int32 {
        do {
            let options = try parseArguments(CommandLine.arguments)
            let transcriber = try await Transcriber.load(
                modelDirectory: options.modelDirectory,
                language: options.language
            )

            switch options.mode {
            case .batch(let audioPath):
                let audioURL = expandedURL(for: audioPath)
                let result = try await transcriber.transcribeFile(at: audioURL)
                for line in BatchFormatter.lines(for: result) {
                    writeStandardOutput(line)
                }
                return 0
            case .stream:
                return await StreamSession(transcriber: transcriber).run()
            }
        } catch CLIControl.help(let message) {
            writeStandardOutput(message)
            return 0
        } catch {
            writeStandardError(error.localizedDescription)
            return 1
        }
    }

    private static func parseArguments(_ arguments: [String]) throws -> CLIOptions {
        var mode: Mode?
        var modelDirectory = defaultModelDirectory
        var language: String?

        var index = 1
        while index < arguments.count {
            let argument = arguments[index]
            switch argument {
            case "--batch":
                index += 1
                guard index < arguments.count else {
                    throw CLIError(message: usage("Missing path after --batch."))
                }
                guard mode == nil else {
                    throw CLIError(message: usage("Choose either --batch or --stream, not both."))
                }
                mode = .batch(audioPath: arguments[index])
            case "--stream":
                guard mode == nil else {
                    throw CLIError(message: usage("Choose either --batch or --stream, not both."))
                }
                mode = .stream
            case "--model-dir":
                index += 1
                guard index < arguments.count else {
                    throw CLIError(message: usage("Missing path after --model-dir."))
                }
                modelDirectory = arguments[index]
            case "--language":
                index += 1
                guard index < arguments.count else {
                    throw CLIError(message: usage("Missing language after --language."))
                }
                language = arguments[index]
            case "--help", "-h":
                throw CLIControl.help(usage(nil))
            default:
                throw CLIError(message: usage("Unknown argument '\(argument)'."))
            }

            index += 1
        }

        guard let mode else {
            throw CLIError(message: usage("Specify exactly one of --batch or --stream."))
        }

        return CLIOptions(mode: mode, modelDirectory: modelDirectory, language: language)
    }

    private static func usage(_ error: String?) -> String {
        let help = """
        Usage:
          parakeet-coreml --batch <audio.wav> [--model-dir <path>] [--language <lang>]
          parakeet-coreml --stream [--model-dir <path>] [--language <lang>]
        """

        if let error {
            return "\(error)\n\n\(help)"
        }

        return help
    }

    private static func expandedURL(for path: String) -> URL {
        URL(fileURLWithPath: (path as NSString).expandingTildeInPath).standardizedFileURL
    }
}

private func writeJSONObject(_ object: [String: Any]) {
    do {
        let data = try JSONSerialization.data(withJSONObject: object)
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data([0x0A]))
        fflush(stdout)
    } catch {
        writeStandardError("Failed to write JSON response: \(error.localizedDescription)")
    }
}

private func writeStandardOutput(_ line: String) {
    FileHandle.standardOutput.write(Data((line + "\n").utf8))
}

private func writeStandardError(_ line: String) {
    FileHandle.standardError.write(Data((line + "\n").utf8))
}
