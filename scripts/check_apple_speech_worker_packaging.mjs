#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

// Update only after a line-by-line review of these complete authority files.
// Structural matches below are necessary but these goldens ensure a copied
// comment or dead duplicate cannot silently replace the active boundary.
const EXPECTED_SOURCE_SHA256 = {
  worker: "65e487ed419c22dab849c7017db222c430c9fceb86f85e6cf25001653f890f08",
  xpc: "bfe9dfc1f015e6825adc02212305af52efe0f2faeff0d79c736cdce4e4c4d2aa",
  swift: "77310730cfa46ac8301c1a65622681005ebf00f0c699f24fdca757e2e02fcef4",
  main: "6c931ff9bac4e041ed2bcb48024313632b8972708c53eee7471808c5a72edc9b",
  acceptanceWorkflow:
    "8ae35943181c6d0f248f580587b894c53db9c30257f307c5aa5264a83f13969d",
  acceptanceHarness:
    "034d8dd5a048b0d8abb6e8a7dfcd82295aa694a43367074b3c4a3a1babd117e0",
};

const files = {
  tauri: JSON.parse(readFileSync("tauri/src-tauri/tauri.macos.conf.json", "utf8")),
  build: readFileSync("scripts/build.sh", "utf8"),
  install: readFileSync("scripts/install-dev-app.sh", "utf8"),
  package: readFileSync("scripts/package-apple-speech-xpc.sh", "utf8"),
  worker: readFileSync("crates/core/src/apple_speech_worker.rs", "utf8"),
  xpc: readFileSync("crates/core/src/macos_graph_xpc.rs", "utf8"),
  swift: readFileSync("crates/core/src/macos_apple_speech_bridge.swift", "utf8"),
  main: readFileSync("tauri/src-tauri/src/main.rs", "utf8"),
  acceptanceWorkflow: readFileSync(
    ".github/workflows/signed-dev-acceptance.yml",
    "utf8",
  ),
  acceptanceHarness: readFileSync(
    "scripts/test_apple_speech_signed_transport.py",
    "utf8",
  ),
  info: readFileSync(
    "crates/cli/assets/minutes-apple-speech-worker-Info.plist",
    "utf8",
  ),
  entitlements: readFileSync(
    "tauri/src-tauri/minutes-apple-speech-worker.entitlements",
    "utf8",
  ),
  // Structural-only (deliberately not golden-hashed): build.rs files change for
  // unrelated reasons and should not force a reseal, but the Swift Concurrency
  // rpaths are load-bearing and must not silently regress.
  coreBuild: readFileSync("crates/core/build.rs", "utf8"),
  cliBuild: readFileSync("crates/cli/build.rs", "utf8"),
};


// Strip comments and string literals before asserting on structure. Without
// this, a required call can be parked in a comment while the real code does
// something else, which is exactly how a demonstrated bypass restored the
// xpc_main block-vs-function-pointer regression with every check still green.
function activeCode(source) {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, " ")
    .replace(/(^|[^:])\/\/[^\n]*/g, "$1 ");
}

function validate(candidate, checkGoldens = true) {
  const errors = [];
  const requireText = (source, value, message) => {
    if (!source.includes(value)) errors.push(message);
  };
  const forbid = (source, pattern, message) => {
    if (pattern.test(source)) errors.push(message);
  };
  const ordered = (source, fragments, message) => {
    let cursor = 0;
    for (const fragment of fragments) {
      const next = source.indexOf(fragment, cursor);
      if (next < 0) {
        errors.push(message);
        return;
      }
      cursor = next + fragment.length;
    }
  };
  const section = (source, start, end, message) => {
    const begin = source.indexOf(start);
    const finish = source.indexOf(end, begin + start.length);
    if (begin < 0 || finish < 0) {
      errors.push(message);
      return "";
    }
    return source.slice(begin, finish);
  };

  if (checkGoldens) {
    for (const [name, expected] of Object.entries(EXPECTED_SOURCE_SHA256)) {
      const observed = createHash("sha256")
        .update(candidate[name])
        .digest("hex");
      if (observed !== expected) {
        errors.push(
          `complete ${name} Apple Speech authority changed; review it and update its golden hash`,
        );
      }
    }
  }

  const appleParent = section(
    candidate.xpc,
    "fn open_apple_speech_authenticated_connection(",
    "pub(crate) fn run(",
    "the Apple Speech parent handshake must remain independently inspectable",
  );
  const appleRun = section(
    candidate.xpc,
    "pub(crate) fn run_apple_speech(",
    "enum ServicePhase",
    "the Apple Speech byte transport must remain independently inspectable",
  );
  const applePhase = section(
    candidate.xpc,
    "enum AppleSpeechServicePhase",
    "fn handle_apple_speech_service_message(",
    "the Apple Speech protocol state machine must remain independently inspectable",
  );
  const appleService = section(
    candidate.xpc,
    'unsafe extern "C" fn apple_speech_service_connection_handler(',
    "#[cfg(test)]",
    "the Apple Speech service authority must remain independently inspectable",
  );

  if (
    !(candidate.tauri.bundle?.externalBin ?? []).includes(
      "bin/minutes-apple-speech-worker",
    )
  ) {
    errors.push("Tauri must stage the Apple Speech worker as an external binary");
  }
  for (const source of [candidate.build, candidate.install]) {
    requireText(
      source,
      "minutes-apple-speech-worker-${HOST_TARGET}",
      "every macOS build path must stage the exact Apple Speech worker",
    );
    requireText(
      source,
      "package-apple-speech-xpc.sh",
      "every macOS build path must package the Apple Speech XPC service",
    );
  }
  for (const value of [
    'SOURCE_WORKER="$APP_BUNDLE/Contents/MacOS/minutes-apple-speech-worker"',
    'XPC_BUNDLE="$APP_BUNDLE/Contents/XPCServices/com.useminutes.apple-speech-worker.xpc"',
    'test ! -e "$SOURCE_WORKER"',
    "--identifier com.useminutes.apple-speech-worker",
    'codesign --verify --strict --verbose=4 "$XPC_BUNDLE"',
    "seal_apple_speech_worker_hash.py",
  ]) {
    requireText(
      candidate.package,
      value,
      `Apple Speech packaging is missing invariant: ${value}`,
    );
  }
  requireText(
    candidate.info,
    "<string>com.useminutes.apple-speech-worker</string>",
    "the XPC Info.plist must bind the dedicated service identifier",
  );
  const entitlementKeys = [
    ...candidate.entitlements.matchAll(/<key>([^<]+)<\/key>/g),
  ].map((match) => match[1]);
  if (
    entitlementKeys.length !== 1 ||
    entitlementKeys[0] !== "com.apple.security.app-sandbox"
  ) {
    errors.push("the Apple Speech worker entitlement allowlist must be App Sandbox only");
  }

  // Without this runtime rpath the weak libswift_Concurrency load resolves to
  // null and the worker aborts in swift_getTypeByMangledName as soon as it
  // constructs a Speech (async) type. Proven on hardware; the analyzer cannot
  // run without it. The shipping worker binary gets its rpath from
  // crates/cli/build.rs (a cargo:rustc-link-arg in minutes-core does not reach
  // a downstream bin); crates/core covers minutes-core's own test/example
  // targets. Both are load-bearing.
  // activeCode strips comments first, so a commented-out or dead-code println!
  // does not satisfy the check (raw source.includes would pass it).
  requireText(
    activeCode(candidate.cliBuild),
    "cargo:rustc-link-arg-bin=minutes-apple-speech-worker=-Wl,-rpath,/usr/lib/swift",
    "the Apple Speech worker bin must add the Swift Concurrency runtime rpath",
  );
  requireText(
    activeCode(candidate.coreBuild),
    "cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift",
    "minutes-core's own targets must add the Swift Concurrency runtime rpath",
  );

  for (const value of [
    "MINUTES_APPLE_SPEECH_WORKER_CDHASH_V1=",
    "current_process_is_trusted_distribution()",
    "peer_requirement_api_available()",
    "MAX_UTTERANCE_SECONDS",
    "samples.iter().any(|sample| !sample.is_finite())",
    "process_private_audio_request",
    "audio.len() != metadata.sample_count.saturating_mul(size_of::<f32>())",
    "RLIMIT_NPROC",
    "RLIMIT_AS",
    "setitimer",
    "parse_private_audio_request",
    "run_signed_transport_acceptance",
    "run_signed_runtime_acceptance",
    "private_audio_parser_accepts_only_exact_finite_pcm_cardinality",
    "private_audio_parser_rejects_magic_metadata_schema_and_budget_attacks",
  ]) {
    requireText(
      candidate.worker,
      value,
      `Apple Speech authority or resource boundary is missing: ${value}`,
    );
  }
  for (const value of [
    "APPLE_SPEECH_XPC_PARENT_REQUEST_LOCK",
    "APPLE_SPEECH_XPC_SETTLEMENT_FAILED",
    "com.useminutes.apple-speech-worker",
    "open_apple_speech_authenticated_connection",
    "set_peer_requirement(connection.object, &requirement)",
    "set_command(begin.0, COMMAND_BEGIN)",
    "handle_apple_speech_service_message",
    "service_request_nonce_matches",
    "xpc_connection_send_barrier",
    "apple_speech_protocol_rejects_replay_reordering_and_premature_transitions",
    "apple_speech_protocol_enforces_aggregate_request_and_response_budgets",
    "apple_speech_abort_is_terminal_from_every_live_phase",
    "apple_speech_one_process_claim_and_disconnect_are_fail_closed",
  ]) {
    requireText(
      candidate.xpc,
      value,
      `Apple Speech authenticated XPC boundary is missing: ${value}`,
    );
  }
  for (const value of [
    "AVAudioPCMBuffer",
    "UnsafeBufferPointer(start: samples",
    "sourceBuffer.floatChannelData",
    "SpeechAnalyzer.bestAvailableAudioFormat",
    "minutes_apple_speech_free_response",
  ]) {
    requireText(
      candidate.swift,
      value,
      `Apple Speech in-memory bridge is missing: ${value}`,
    );
  }
  for (const source of [
    candidate.worker,
    candidate.xpc,
    candidate.swift,
    candidate.package,
  ]) {
    forbid(
      source,
      /POSIX_SPAWN_START_SUSPENDED|SIGCONT|attest_and_resume/,
      "the rejected suspended-spawn primitive must not return",
    );
  }
  forbid(
    candidate.swift,
    /AVAudioFile|audioPath|temporaryDirectory|NSTemporaryDirectory|FileHandle|FileManager|fileURLWithPath|\.write\(to:/,
    "the private Apple Speech bridge must not open or create a named audio file",
  );
  forbid(
    candidate.worker,
    /Command::new|BoundExecutable|executable:|arguments:|environment:|File::create|File::open|OpenOptions|NamedTempFile|tempfile::|std::fs::write|write_wav/,
    "the dedicated Apple Speech worker must not become a generic process launcher",
  );
  forbid(
    `${applePhase}\n${appleService}`,
    /File::create|File::open|OpenOptions|NamedTempFile|tempfile::|std::fs::write|write_wav/,
    "the Apple Speech XPC state machine must not gain a named-file surface",
  );
  forbid(
    appleParent,
    /COMMAND_CHUNK|DATA_KEY/,
    "no Apple Speech content may be constructed in the authentication handshake",
  );
  ordered(
    appleParent,
    [
      'identifier \\"com.useminutes.apple-speech-worker\\" and cdhash',
      "set_peer_requirement(connection.object, &requirement)",
      "xpc_connection_resume(connection.object)",
      "set_command(begin.0, COMMAND_BEGIN)",
      "connection.send_with_reply(begin.0, deadline)",
    ],
    "the exact peer requirement and content-free handshake must precede Apple Speech bytes",
  );
  ordered(
    appleRun,
    [
      "open_apple_speech_authenticated_connection(",
      "set_command(chunk.0, COMMAND_CHUNK)",
      "xpc_dictionary_set_data(",
      "connection.settle(outcome.is_err(), deadline)",
      "APPLE_SPEECH_XPC_SETTLEMENT_FAILED.store",
    ],
    "Apple Speech bytes must follow authentication and retain fail-closed settlement",
  );
  for (const value of [
    "checked_add(data.len())",
    "crate::apple_speech_worker::MAX_REQUEST_BYTES",
    "sequence != *next_sequence",
    "offset != *next_offset",
  ]) {
    requireText(
      applePhase,
      value,
      `Apple Speech protocol framing or budget is missing: ${value}`,
    );
  }
  ordered(
    appleService,
    [
      'unsafe extern "C" fn apple_speech_service_connection_handler(',
      "service_parent_requirement()",
      "set_peer_requirement(peer, &requirement)",
      "xpc_connection_resume(peer)",
      "pub fn run_apple_speech_service_main()",
      "APPLE_SPEECH_SERVICE_NONCE.set(service_nonce)",
      "xpc_main(apple_speech_service_connection_handler)",
    ],
    "the worker must use the C connection-handler ABI and authenticate its signed parent before receiving a message",
  );
  requireText(
    candidate.xpc,
    'type XpcConnectionHandler = unsafe extern "C" fn(XpcObject);',
    "xpc_main must use the plain C connection-handler ABI",
  );
  requireText(
    candidate.xpc,
    "fn xpc_main(handler: XpcConnectionHandler) -> !;",
    "the xpc_main declaration must not reinterpret an Objective-C block as a C callback",
  );
  forbid(
    candidate.xpc,
    /fn xpc_main\(handler: &Block/,
    "xpc_main must never receive an Objective-C block",
  );

  // The ABI is only safe if exactly one xpc_main call exists per named C
  // connection handler, and nothing casts a block into that callback type.
  // Text assertions alone are satisfiable by dead code, so count the real
  // call sites in comment-stripped source.
  {
    const active = activeCode(candidate.xpc);
    const handlers = (
      active.match(/unsafe extern "C" fn \w*service_connection_handler\(/g) ?? []
    ).length;
    const mains = (active.match(/\bxpc_main\(/g) ?? []).length;
    // one declaration plus one call per handler
    if (mains !== handlers + 1) {
      errors.push(
        `xpc_main call sites (${mains}) must equal one per C connection handler plus its declaration (${handlers + 1})`,
      );
    }
    // One transmute is legitimate: turning a dlsym address into the
    // peer-requirement function pointer. Any other transmute, or any cast to
    // the connection-handler type, could smuggle a block back into xpc_main.
    const allowedTransmute = active.slice(
      active.indexOf("fn load_peer_requirement("),
      active.indexOf("fn set_peer_requirement("),
    );
    const totalTransmutes = (active.match(/\btransmute\b/g) ?? []).length;
    const allowedTransmutes = (allowedTransmute.match(/\btransmute\b/g) ?? []).length;
    if (totalTransmutes !== allowedTransmutes || allowedTransmutes !== 1) {
      errors.push(
        "the only transmute in the XPC authority must be the peer-requirement dlsym lookup",
      );
    }
    if (/as\s+XpcConnectionHandler/.test(active)) {
      errors.push("a connection handler must be a named C function, not a cast");
    }
    if (/#\[link_name\s*=\s*"xpc_main"\]/.test(active)) {
      errors.push("xpc_main must not be re-declared under an alias");
    }
  }
  for (const value of [
    "service_request_nonce_matches",
    "claim_service_process",
    "xpc_connection_send_barrier",
  ]) {
    requireText(
      appleService,
      value,
      `Apple Speech one-shot service settlement is missing: ${value}`,
    );
  }
  requireText(
    candidate.main,
    "install_apple_speech_worker_service(service)",
    "the desktop must bind the embedded authority before normal startup",
  );
  for (const value of [
    "--apple-speech-transport-acceptance",
    "run_signed_transport_acceptance()",
    "--apple-speech-runtime-acceptance",
    "run_signed_runtime_acceptance()",
    "acceptance accepts no caller-supplied input",
  ]) {
    requireText(
      candidate.main,
      value,
      `the signed non-product acceptance route is missing: ${value}`,
    );
  }
  for (const value of [
    "run-signed-runtime:",
    "needs:",
    "- sign-reviewed-artifact",
    "test_apple_speech_signed_transport.py",
    "signed-runtime-provenance.json",
  ]) {
    requireText(
      candidate.acceptanceWorkflow,
      value,
      `the no-secret signed runtime job is missing: ${value}`,
    );
  }
  for (const value of [
    "SameUidOpenHolder",
    "os.O_NOFOLLOW",
    "raw_f32",
    "pcm_i16",
    "runtime_fixture_patterns",
    "apple-speech-signed-runtime=accepted",
    "signedWorkerRuntime",
    "namedAudioCanaryObserved",
    "productGateExpectedClosed",
  ]) {
    requireText(
      candidate.acceptanceHarness,
      value,
      `the hostile same-UID open-holder harness is missing: ${value}`,
    );
  }
  return errors;
}

if (process.argv.includes("--self-test")) {
  const mutations = [
    ["worker Concurrency rpath commented out", "cliBuild", (value) =>
      value.replace(
        '"cargo:rustc-link-arg-bin=minutes-apple-speech-worker=-Wl,-rpath,/usr/lib/swift"',
        '// "cargo:rustc-link-arg-bin=minutes-apple-speech-worker=-Wl,-rpath,/usr/lib/swift"',
      ), false],
    ["worker Concurrency rpath removed", "cliBuild", (value) =>
      value.replace(
        '"cargo:rustc-link-arg-bin=minutes-apple-speech-worker=-Wl,-rpath,/usr/lib/swift"',
        '"cargo:rustc-link-arg-bin=minutes-apple-speech-worker=-Wl,-sectcreate,__TEXT,__foo,/dev/null"',
      ), false],
    ["core Concurrency rpath commented out", "coreBuild", (value) =>
      value.replace(
        '"cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift"',
        '// "cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift"',
      ), false],
    ["xpc_main aliased to a block declaration", "xpc", (value) =>
      value.replace(
        "fn xpc_main(handler: XpcConnectionHandler) -> !;",
        'fn xpc_main(handler: XpcConnectionHandler) -> !;\n    #[link_name = "xpc_main"]\n    fn xpc_main_compat(handler: &Block<dyn Fn(XpcObject)>) -> !;',
      ), false],
    ["connection handler smuggled in by transmute", "xpc", (value) =>
      value.replace(
        "unsafe { xpc_main(apple_speech_service_connection_handler) }",
        "let shim: XpcConnectionHandler = unsafe { std::mem::transmute(0usize) };\n    unsafe { xpc_main(shim) }",
      ), false],
    ["generic launcher", "worker", (value) => `${value}\nCommand::new(\"helper\");`, false],
    [
      "Rust named audio file",
      "worker",
      (value) => `${value}\nstd::fs::File::create(\"utterance.wav\");`,
      false,
    ],
    ["Swift named audio file", "swift", (value) => `${value}\nlet f: AVAudioFile? = nil`, false],
    [
      "service left in MacOS",
      "package",
      (value) => value.replace('test ! -e "$SOURCE_WORKER"', 'test -e "$SOURCE_WORKER"'),
      false,
    ],
    [
      "missing service peer requirement",
      "xpc",
      (value) =>
        value.replaceAll(
          "set_peer_requirement(connection.object, &requirement)",
          "drop(requirement)",
        ),
      false,
    ],
    [
      "XPC main callback ABI regressed to a block",
      "xpc",
      (value) =>
        value.replace(
          "fn xpc_main(handler: XpcConnectionHandler) -> !;",
          "fn xpc_main(handler: &Block<dyn Fn(XpcObject)>) -> !;",
        ),
      false,
    ],
    [
      "content before authentication",
      "xpc",
      (value) =>
        value.replaceAll(
          "set_peer_requirement(connection.object, &requirement)?;",
          "set_command(OwnedXpc::dictionary()?.0, COMMAND_CHUNK);\n    set_peer_requirement(connection.object, &requirement)?;",
        ),
      false,
    ],
    [
      "missing reciprocal parent authentication",
      "xpc",
      (value) =>
        value.replaceAll(
          "let Ok(requirement) = service_parent_requirement() else {",
          "let Ok(requirement) = CString::new(\"identifier true\") else {",
        ),
      false,
    ],
    [
      "missing service nonce",
      "xpc",
      (value) =>
        value.replaceAll(
          "service_request_nonce_matches(message, &message_service_nonce)",
          "true",
        ),
      false,
    ],
    [
      "missing exact framing",
      "worker",
      (value) =>
        value.replace(
          "audio.len() != metadata.sample_count.saturating_mul(size_of::<f32>())",
          "false",
        ),
      false,
    ],
    [
      "missing request budget",
      "xpc",
      (value) =>
        value.replaceAll(
          "crate::apple_speech_worker::MAX_REQUEST_BYTES",
          "usize::MAX",
        ),
      false,
    ],
    [
      "missing settlement poison",
      "xpc",
      (value) =>
        value.replaceAll(
          "APPLE_SPEECH_XPC_SETTLEMENT_FAILED.store(true, Ordering::Release);",
          "return Err(settlement);",
        ),
      false,
    ],
    [
      "comment-only invariant drift",
      "worker",
      (value) => value.replace("Exact-capability boundary", "Exact capability boundary"),
      true,
    ],
  ];
  for (const [name, key, mutate, requireGolden] of mutations) {
    const candidate = { ...files, [key]: mutate(files[key]) };
    if (validate(candidate, requireGolden).length === 0) {
      throw new Error(`self-test mutation was accepted: ${name}`);
    }
  }
  process.stdout.write("Apple Speech worker packaging self-test passed\n");
  process.exit(0);
}

const errors = validate(files);
if (errors.length > 0) {
  for (const error of errors) process.stderr.write(`ERROR: ${error}\n`);
  process.exit(1);
}
process.stdout.write("Apple Speech worker packaging checks passed\n");
