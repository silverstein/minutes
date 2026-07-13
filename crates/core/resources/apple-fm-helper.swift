// apple-fm-helper — on-device summarization via Apple Foundation Models.
//
// Compiled on demand by minutes-core (see crates/core/src/apple_fm.rs), the
// same lifecycle as apple-speech-helper.swift. Requires macOS 26+ with Apple
// Intelligence enabled; on anything older the helper still compiles and
// reports runtimeSupported=false instead of failing.
//
// Contract (JSON on stdout, one object per invocation):
//   apple-fm-helper capabilities
//     -> {"kind":"capabilities","schemaVersion":1,...}
//   apple-fm-helper generate --input-file <path>
//     input file: {"systemPrompt":"...","prompt":"..."}
//     -> {"kind":"generation","schemaVersion":1,"text":...,"error":...}
//
// The prompt travels via a caller-owned 0600 temp file, never argv, so
// transcript content cannot appear in the process list.

import Foundation

#if canImport(FoundationModels)
import FoundationModels
#endif

struct CapabilityReport: Codable {
    let kind: String
    let schemaVersion: Int
    let osVersion: String
    let runtimeSupported: Bool
    let availability: String
    let reason: String?
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

func osVersionString() -> String {
    let v = ProcessInfo.processInfo.operatingSystemVersion
    return "\(v.majorVersion).\(v.minorVersion).\(v.patchVersion)"
}

func emit<T: Codable>(_ value: T) {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys]
    if let data = try? encoder.encode(value),
        let text = String(data: data, encoding: .utf8)
    {
        print(text)
    }
}

func capabilityReport() -> CapabilityReport {
    #if canImport(FoundationModels)
    if #available(macOS 26.0, *) {
        let model = SystemLanguageModel.default
        switch model.availability {
        case .available:
            return CapabilityReport(
                kind: "capabilities", schemaVersion: 1, osVersion: osVersionString(),
                runtimeSupported: true, availability: "available", reason: nil)
        case .unavailable(let reason):
            return CapabilityReport(
                kind: "capabilities", schemaVersion: 1, osVersion: osVersionString(),
                runtimeSupported: true, availability: "unavailable",
                reason: String(describing: reason))
        }
    }
    #endif
    return CapabilityReport(
        kind: "capabilities", schemaVersion: 1, osVersion: osVersionString(),
        runtimeSupported: false, availability: "unavailable",
        reason: "FoundationModels requires macOS 26 or newer")
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

@main
struct AppleFmHelper {
    static func main() async {
        var args = Array(CommandLine.arguments.dropFirst())
        let command = args.isEmpty ? "capabilities" : args.removeFirst()

        switch command {
        case "capabilities", "--capabilities":
            emit(capabilityReport())
        case "generate":
            var inputFile: String? = nil
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
        default:
            FileHandle.standardError.write(
                Data("unknown command: \(command)\n".utf8))
            exit(2)
        }
    }
}
