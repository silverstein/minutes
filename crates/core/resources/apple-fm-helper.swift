// apple-fm-helper — on-device generation via Apple Foundation Models.
//
// Compiled on demand by minutes-core (see crates/core/src/apple_fm.rs), the
// same lifecycle as apple-speech-helper.swift. Requires macOS 26+ with Apple
// Intelligence enabled; on anything older the helper still compiles and
// reports runtimeSupported=false instead of failing.
//
// One-shot summarization contract (JSON on stdout):
//   apple-fm-helper capabilities
//   apple-fm-helper generate --input-file <path>
//
// Long-lived copilot contract (NDJSON on stdin/stdout):
//   apple-fm-helper copilot-server
//   <- {"kind":"prewarm","schemaVersion":1,"id":"...","systemPrompt":"..."}
//   -> {"kind":"prewarmed",...}
//   <- {"kind":"generate","schemaVersion":1,"id":"...","prompt":"..."}
//   -> zero or more {"kind":"snapshot",...}
//   -> {"kind":"completed",...}
//   <- {"kind":"cancel","schemaVersion":1,"id":"..."}
//   -> {"kind":"cancelled",...}
//
// The one-shot prompt travels through a caller-owned 0600 temp file. Copilot
// prompts travel through the long-lived helper's stdin. Neither appears in
// argv, and the Foundation Models framework remains entirely on-device.

import Foundation

#if canImport(FoundationModels)
import FoundationModels
#endif

let copilotProtocolVersion = 1
let copilotPromptVersion = 1

struct CapabilityReport: Codable {
    let kind: String
    let schemaVersion: Int
    let osVersion: String
    let runtimeSupported: Bool
    let availability: String
    let reason: String?
    let replayGateKey: String
}

struct GenerationInput: Codable {
    let systemPrompt: String
    let prompt: String
}

struct GenerationResult: Codable {
    let kind: String
    let schemaVersion: Int
    let text: String?
    let error: String?
    let elapsedMs: UInt64
}

struct CopilotCommand: Decodable {
    let kind: String
    let schemaVersion: Int
    let id: String
    let systemPrompt: String?
    let prompt: String?
}

struct CopilotSnapshot: Codable {
    let kind: String?
    let text: String?
    let sourceChip: String?
}

struct CopilotEvent: Encodable {
    let kind: String
    let schemaVersion: Int
    let id: String
    let snapshot: CopilotSnapshot?
    let error: String?
    let osVersion: String
    let replayGateKey: String
}

func osVersionString() -> String {
    let version = ProcessInfo.processInfo.operatingSystemVersion
    return "\(version.majorVersion).\(version.minorVersion).\(version.patchVersion)"
}

func replayGateKey() -> String {
    "apple-fm-copilot/prompt-v\(copilotPromptVersion)/protocol-v\(copilotProtocolVersion)/macos-\(osVersionString())"
}

func emit<T: Encodable>(_ value: T) {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys]
    guard let data = try? encoder.encode(value) else { return }
    var line = data
    line.append(0x0A)
    FileHandle.standardOutput.write(line)
}

func emitCopilot(
    kind: String, id: String, snapshot: CopilotSnapshot? = nil, error: String? = nil
) {
    emit(
        CopilotEvent(
            kind: kind,
            schemaVersion: copilotProtocolVersion,
            id: id,
            snapshot: snapshot,
            error: error,
            osVersion: osVersionString(),
            replayGateKey: replayGateKey()))
}

func capabilityReport() -> CapabilityReport {
    #if canImport(FoundationModels)
    if #available(macOS 26.0, *) {
        let model = SystemLanguageModel.default
        switch model.availability {
        case .available:
            return CapabilityReport(
                kind: "capabilities", schemaVersion: 1, osVersion: osVersionString(),
                runtimeSupported: true, availability: "available", reason: nil,
                replayGateKey: replayGateKey())
        case .unavailable(let reason):
            return CapabilityReport(
                kind: "capabilities", schemaVersion: 1, osVersion: osVersionString(),
                runtimeSupported: true, availability: "unavailable",
                reason: String(describing: reason), replayGateKey: replayGateKey())
        }
    }
    #endif
    return CapabilityReport(
        kind: "capabilities", schemaVersion: 1, osVersion: osVersionString(),
        runtimeSupported: false, availability: "unavailable",
        reason: "FoundationModels requires macOS 26 or newer",
        replayGateKey: replayGateKey())
}

func runGenerate(inputFile: String) async {
    let started = DispatchTime.now()
    func elapsedMs() -> UInt64 {
        (DispatchTime.now().uptimeNanoseconds - started.uptimeNanoseconds) / 1_000_000
    }
    func fail(_ message: String) {
        emit(
            GenerationResult(
                kind: "generation", schemaVersion: 1, text: nil, error: message,
                elapsedMs: elapsedMs()))
    }

    guard let data = FileManager.default.contents(atPath: inputFile),
        let input = try? JSONDecoder().decode(GenerationInput.self, from: data)
    else {
        fail("could not read or parse generation input file")
        return
    }

    #if canImport(FoundationModels)
    if #available(macOS 26.0, *) {
        let model = SystemLanguageModel.default
        guard case .available = model.availability else {
            fail("Apple Intelligence model unavailable on this system")
            return
        }
        do {
            let session = LanguageModelSession(instructions: input.systemPrompt)
            let options = GenerationOptions(temperature: 0.3)
            let response = try await session.respond(to: input.prompt, options: options)
            emit(
                GenerationResult(
                    kind: "generation", schemaVersion: 1, text: response.content,
                    error: nil, elapsedMs: elapsedMs()))
        } catch {
            fail("generation failed: \(error)")
        }
        return
    }
    #endif
    fail("FoundationModels requires macOS 26 or newer")
}

func runUnavailableCopilotServer(reason: String) {
    while let line = readLine() {
        guard let data = line.data(using: .utf8),
            let command = try? JSONDecoder().decode(CopilotCommand.self, from: data)
        else { continue }
        emitCopilot(kind: "error", id: command.id, error: reason)
    }
}

#if canImport(FoundationModels)
@available(macOS 26.0, *)
@Generable
enum CopilotNudgeKind {
    case say
    case ask
    case clarify
    case hold
    case watch
}

@available(macOS 26.0, *)
@Generable
struct CopilotNudge {
    @Guide(description: "The nudge category")
    let kind: CopilotNudgeKind

    @Guide(description: "Presentation-ready guidance, preferably no more than 24 words")
    let text: String

    @Guide(description: "A short evidence label copied or closely paraphrased from the transcript")
    let sourceChip: String
}

@available(macOS 26.0, *)
func nudgeKindName(_ kind: CopilotNudgeKind) -> String {
    switch kind {
    case .say: return "Say"
    case .ask: return "Ask"
    case .clarify: return "Clarify"
    case .hold: return "Hold"
    case .watch: return "Watch"
    }
}

@available(macOS 26.0, *)
actor CopilotServer {
    private var session: LanguageModelSession?
    private var systemPrompt: String?
    private var currentID: String?
    private var currentTask: Task<Void, Never>?

    func prewarm(id: String, requestedSystemPrompt: String?) {
        guard currentTask == nil else {
            emitCopilot(kind: "error", id: id, error: "a copilot generation is already active")
            return
        }
        guard let requestedSystemPrompt, !requestedSystemPrompt.isEmpty else {
            emitCopilot(kind: "error", id: id, error: "prewarm requires systemPrompt")
            return
        }
        if session == nil || systemPrompt != requestedSystemPrompt {
            session = LanguageModelSession(instructions: requestedSystemPrompt)
            systemPrompt = requestedSystemPrompt
        }
        session?.prewarm(promptPrefix: nil)
        emitCopilot(kind: "prewarmed", id: id)
    }

    func generate(id: String, prompt: String?) {
        guard currentTask == nil else {
            emitCopilot(kind: "error", id: id, error: "a copilot generation is already active")
            return
        }
        guard session != nil else {
            emitCopilot(kind: "error", id: id, error: "copilot session must be prewarmed first")
            return
        }
        guard let prompt, !prompt.isEmpty else {
            emitCopilot(kind: "error", id: id, error: "generate requires prompt")
            return
        }

        currentID = id
        currentTask = Task { [weak self] in
            guard let self else { return }
            await self.runGeneration(id: id, prompt: prompt)
        }
    }

    func cancel(id: String) {
        guard currentID == id, let currentTask else {
            emitCopilot(kind: "cancelled", id: id)
            return
        }
        currentTask.cancel()
    }

    func shutdown() async {
        let task = currentTask
        task?.cancel()
        await task?.value
    }

    private func finish(id: String) {
        if currentID == id {
            currentID = nil
            currentTask = nil
        }
    }

    private func runGeneration(id: String, prompt: String) async {
        guard let session else {
            finish(id: id)
            emitCopilot(kind: "error", id: id, error: "copilot session is unavailable")
            return
        }

        var lastSnapshot: CopilotSnapshot?
        do {
            let options = GenerationOptions(temperature: 0.2)
            let stream = session.streamResponse(
                to: prompt, generating: CopilotNudge.self, options: options)
            for try await partialResponse in stream {
                try Task.checkCancellation()
                let content = partialResponse.content
                let snapshot = CopilotSnapshot(
                    kind: content.kind.map(nudgeKindName),
                    text: content.text,
                    sourceChip: content.sourceChip)
                lastSnapshot = snapshot
                emitCopilot(kind: "snapshot", id: id, snapshot: snapshot)
            }
            try Task.checkCancellation()
            guard let snapshot = lastSnapshot,
                snapshot.kind != nil, snapshot.text != nil, snapshot.sourceChip != nil
            else {
                finish(id: id)
                emitCopilot(
                    kind: "error", id: id,
                    error: "Apple Foundation Models completed without a full nudge snapshot")
                return
            }
            finish(id: id)
            emitCopilot(kind: "completed", id: id, snapshot: snapshot)
        } catch {
            let wasCancelled = Task.isCancelled || error is CancellationError
            finish(id: id)
            if wasCancelled {
                emitCopilot(kind: "cancelled", id: id)
            } else {
                emitCopilot(kind: "error", id: id, error: "generation failed: \(error)")
            }
        }
    }
}

@available(macOS 26.0, *)
func runAvailableCopilotServer() async {
    let server = CopilotServer()
    while let line = readLine() {
        guard let data = line.data(using: .utf8),
            let command = try? JSONDecoder().decode(CopilotCommand.self, from: data)
        else { continue }
        guard command.schemaVersion == copilotProtocolVersion else {
            emitCopilot(
                kind: "error", id: command.id,
                error: "unsupported copilot helper protocol \(command.schemaVersion)")
            continue
        }
        switch command.kind {
        case "prewarm":
            await server.prewarm(id: command.id, requestedSystemPrompt: command.systemPrompt)
        case "generate":
            await server.generate(id: command.id, prompt: command.prompt)
        case "cancel":
            await server.cancel(id: command.id)
        default:
            emitCopilot(kind: "error", id: command.id, error: "unknown command: \(command.kind)")
        }
    }
    await server.shutdown()
}
#endif

func runCopilotServer() async {
    let capability = capabilityReport()
    guard capability.runtimeSupported, capability.availability == "available" else {
        runUnavailableCopilotServer(
            reason: capability.reason ?? "Apple Intelligence model unavailable on this system")
        return
    }

    #if canImport(FoundationModels)
    if #available(macOS 26.0, *) {
        await runAvailableCopilotServer()
        return
    }
    #endif
    runUnavailableCopilotServer(reason: "FoundationModels requires macOS 26 or newer")
}

@main
struct AppleFmHelper {
    static func main() async {
        var args = Array(CommandLine.arguments.dropFirst())
        let command = args.isEmpty ? "capabilities" : args.removeFirst()

        switch command {
        case "capabilities", "--capabilities":
            emit(capabilityReport())
        case "generate":
            var inputFile: String?
            var index = 0
            while index < args.count {
                if args[index] == "--input-file", index + 1 < args.count {
                    inputFile = args[index + 1]
                    index += 2
                } else {
                    index += 1
                }
            }
            guard let inputFile else {
                emit(
                    GenerationResult(
                        kind: "generation", schemaVersion: 1, text: nil,
                        error: "generate requires --input-file <path>", elapsedMs: 0))
                exit(2)
            }
            await runGenerate(inputFile: inputFile)
        case "copilot-server":
            await runCopilotServer()
        default:
            FileHandle.standardError.write(Data("unknown command: \(command)\n".utf8))
            exit(2)
        }
    }
}
