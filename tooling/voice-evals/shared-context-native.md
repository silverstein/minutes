# Shared-window native qualification

This fixture is deliberately separate from a person's browser or draft. It
contains a slider, a derived output and a timed document-title change. It sends
nothing, writes no user document and exits after 28 seconds.

Ask for a short foreground window before launching it. Do not run this while the
user is recording or working in another focused app. Do not reset TCC or replace
the production Minutes app to make a test pass. Terminal qualification and the
signed Minutes Dev application's acceptance are different receipts.

From the source worktree, compile the test executable with the pinned Rust
toolchain, reusing the approved target directory and limiting build workers:

```sh
CARGO_BUILD_JOBS=2 ~/.cargo/bin/cargo test -p minutes-core --no-default-features --features voice-live --lib --no-run
```

Use the exact executable printed by that command, not a guessed hash. Build the
disposable app bundle (the following commands run on macOS):

```sh
fixture_root=$(mktemp -d /tmp/minutes-shared-context.XXXXXX)
fixture="$fixture_root/Minutes Shared Context Fixture.app"
mkdir -p "$fixture/Contents/MacOS"
cp tooling/voice-evals/SharedContextFixture.plist "$fixture/Contents/Info.plist"
swiftc tooling/voice-evals/SharedContextFixture.swift -o "$fixture/Contents/MacOS/fixture" -framework Cocoa
```

Only after the user grants the foreground window, open that app and immediately
run the exact test executable with:

```sh
MINUTES_SHARED_CONTEXT_FIXTURE=1 "$test_executable" voice_live::shared_context::tests::native_events_capture_changes_and_document_switch_revokes --exact --ignored --nocapture
```

The test must start well before the fixture's eight-second change. It checks:

- Exact frontmost bundle and initial `Revenue: 50` accessibility evidence.
- A PNG of that one identified window, not the whole desktop.
- A notification-triggered change to `Revenue: 150`, not merely a timer refresh.
- The document-title change revoking the grant, then explicit stop refusing reads.

A missing Accessibility or Screen Recording permission, ambiguous window identity
or incomplete accessibility tree is a failure/limitation, not a successful skip.
Capture the exact source commit, executable hash, responsible app, permissions
context, duration and result in the acceptance contract. Remove only the named
fixture directory after the app has exited. No screenshot or microphone sample
should be retained as part of routine receipts.

Additional acceptance is still required for real browser selection, stale-write
refusal, window focus loss, expiry, spoken park/restart/resume and prototype
redirection. This single fixture cannot qualify those workflows or hidden browser
document changes that expose neither a changed title nor an AXDocument identity.
