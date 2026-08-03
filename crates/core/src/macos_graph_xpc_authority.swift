import Foundation
import Security

private let minutesDesktopSigningRequirement =
    #"anchor apple generic and certificate leaf[subject.OU] = "63TMLKT8HN" and (identifier "com.useminutes.desktop" or identifier "com.useminutes.desktop.dev")"#
private let minutesTeamSigningRequirement =
    #"anchor apple generic and certificate leaf[subject.OU] = "63TMLKT8HN""#

private func codeDirectoryHash(_ code: SecStaticCode) throws -> Data {
    var information: CFDictionary?
    guard SecCodeCopySigningInformation(
        code,
        SecCSFlags(),
        &information
    ) == errSecSuccess,
    let values = information as? [CFString: Any],
    let hash = values[kSecCodeInfoUnique] as? Data,
    hash.count == 20 else {
        throw POSIXError(.EACCES)
    }
    return hash
}

private func validateSealedAuthorityBundle(
    _ bundlePath: String,
    currentExecutablePath: String,
    runningParentCodeDirectoryHash: Data
) throws {
    var requirement: SecRequirement?
    guard SecRequirementCreateWithString(
        minutesDesktopSigningRequirement as CFString,
        SecCSFlags(),
        &requirement
    ) == errSecSuccess,
    let requirement else {
        throw POSIXError(.EACCES)
    }
    var staticCode: SecStaticCode?
    let createStatus = SecStaticCodeCreateWithPath(
        URL(fileURLWithPath: bundlePath) as CFURL,
        SecCSFlags(),
        &staticCode
    )
    guard createStatus == errSecSuccess, let staticCode else {
        throw POSIXError(.EACCES)
    }
    let flags = SecCSFlags(
        rawValue: kSecCSCheckAllArchitectures
            | kSecCSCheckNestedCode
            | kSecCSStrictValidate
            | kSecCSRestrictSymlinks
    )
    guard SecStaticCodeCheckValidity(
        staticCode,
        flags,
        requirement
    ) == errSecSuccess else {
        throw POSIXError(.EACCES)
    }

    // Tie the validated on-disk bundle back to the already-running parent.
    // This prevents replacing the whole package with an older legitimately
    // signed Minutes release before the helper request.
    var onDiskParent: SecStaticCode?
    guard SecStaticCodeCreateWithPath(
        URL(fileURLWithPath: currentExecutablePath) as CFURL,
        SecCSFlags(),
        &onDiskParent
    ) == errSecSuccess,
    let onDiskParent,
    try codeDirectoryHash(onDiskParent) == runningParentCodeDirectoryHash else {
        throw POSIXError(.EACCES)
    }
}

@_cdecl("minutes_current_process_is_trusted_distribution")
public func minutesCurrentProcessIsTrustedDistribution() -> Int32 {
    var requirement: SecRequirement?
    guard SecRequirementCreateWithString(
        minutesTeamSigningRequirement as CFString,
        SecCSFlags(),
        &requirement
    ) == errSecSuccess,
    let requirement else {
        return -1
    }
    var liveCode: SecCode?
    guard SecCodeCopySelf(SecCSFlags(), &liveCode) == errSecSuccess,
          let liveCode else {
        return -1
    }
    let status = SecCodeCheckValidity(liveCode, SecCSFlags(), requirement)
    if status == errSecSuccess {
        return 1
    }
    // Only errSecCSReqFailed is a definitive "this process is not a trusted
    // distribution build". Any other status means the evaluation itself could
    // not complete, and the caller must not read that as a development build.
    return status == errSecCSReqFailed ? 0 : -1
}

/// Validate the signed application bundle that seals the embedded XPC
/// service's expected CDHash. Rust separately installs that exact CDHash as
/// the XPC peer code-signing requirement before the content-free handshake.
@_cdecl("minutes_validate_graph_authority_bundle")
public func minutesValidateGraphAuthorityBundle(
    _ authorityBundlePath: UnsafePointer<CChar>,
    _ currentExecutablePath: UnsafePointer<CChar>,
    _ runningParentCodeDirectoryHash: UnsafePointer<UInt8>,
    _ runningParentCodeDirectoryHashLength: Int
) -> Int32 {
    return autoreleasepool {
        do {
            guard runningParentCodeDirectoryHashLength == 20 else {
                throw POSIXError(.EINVAL)
            }
            let parentCodeDirectoryHash = Data(
                bytes: runningParentCodeDirectoryHash,
                count: runningParentCodeDirectoryHashLength
            )
            try validateSealedAuthorityBundle(
                String(cString: authorityBundlePath),
                currentExecutablePath: String(cString: currentExecutablePath),
                runningParentCodeDirectoryHash: parentCodeDirectoryHash
            )
            return 0
        } catch let error as POSIXError {
            return Int32(error.code.rawValue)
        } catch {
            return Int32(EACCES)
        }
    }
}

/// Apple Speech uses the same sealed-parent validation as the graph service,
/// but keeps a separate exported symbol so Rust cannot accidentally validate
/// one authority while connecting to the other service name.
@_cdecl("minutes_validate_apple_speech_authority_bundle")
public func minutesValidateAppleSpeechAuthorityBundle(
    _ authorityBundlePath: UnsafePointer<CChar>,
    _ currentExecutablePath: UnsafePointer<CChar>,
    _ runningParentCodeDirectoryHash: UnsafePointer<UInt8>,
    _ runningParentCodeDirectoryHashLength: Int
) -> Int32 {
    minutesValidateGraphAuthorityBundle(
        authorityBundlePath,
        currentExecutablePath,
        runningParentCodeDirectoryHash,
        runningParentCodeDirectoryHashLength
    )
}
