# CI timing, November 2026

Measured 2026-11-09 from Actions jobs and timestamped job logs. Minutes are job start to job completion, excluding queue time. Compile/check time is the sum of Cargo's reported `Finished` durations (including linking); execution is the sum of suite summaries over 10 seconds, excluding child-test summaries. These are diagnostic breakdowns, not a benchmark on identical hardware.

## Baseline sample

All five sampled runs below were green. Values are minutes; cache sizes are compressed restore sizes from the logs. M means miss, H means exact hit.

| Run | Event | macOS wall / compile / execution | Windows wall / compile / execution | Linux wall | Cache Mac / Windows / Linux |
| --- | --- | --- | --- | --- | --- |
| [42269098034](https://github.com/silverstein/minutes/actions/runs/42269098034) | main, Nov 6 | 62.6 / 38.9 / 20.7 | 46.0 / 21.8 / 20.3 | 16.9 | M / M / H 2,285 MiB |
| [42569718457](https://github.com/silverstein/minutes/actions/runs/42569718457) | PR, Nov 9 | 38.6 / 16.3 / 19.3 | 52.1 / 27.6 / 23.4 | 17.8 | H 3,786 MiB / M / H 2,285 MiB |
| [42576089540](https://github.com/silverstein/minutes/actions/runs/42576089540) | main, Nov 9 | 44.9 / 21.4 / 20.2 | 36.9 / 17.8 / 16.5 | 15.6 | H 3,786 MiB / H 1,977 MiB / H 2,285 MiB |
| [42639660062](https://github.com/silverstein/minutes/actions/runs/42639660062) | PR, Nov 9 | 65.0 / 41.9 / 21.7 | 43.8 / 26.2 / 16.6 | 20.7 | M / M / M |
| [42651976389](https://github.com/silverstein/minutes/actions/runs/42651976389) | main, Nov 9 | 56.5 / 33.7 / 20.1 | 55.1 / 26.9 / 22.7 | 21.5 | M / M / M |

The release-prep PR's [latest jobs](https://github.com/silverstein/minutes/actions/runs/42652164389) were also green in the test matrix (the overall run failed elsewhere): macOS 66.8 minutes, Windows 47.7, Linux 22.7. This differs from the earlier attempt quoted in the request; use the linked run/attempt rather than mixing attempts.

PR hits including that release-prep run: macOS 1/3, Windows 0/3, Linux 2/3. The five green-run main/PR sample has Mac 2/5, Windows 1/5, Linux 3/5. Restore cost when warm: Mac 109-110 seconds, Windows 97 seconds, Linux 36-74 seconds. Misses return in about a second. An exact lockfile-only key cannot reuse an older lockfile's cache, and multi-GB entries compete for 10 GB. An API snapshot at 18:22Z had no main macOS test cache. Missing keys alone do not distinguish eviction, runner-image turnover and not-yet-seeded locks.

## Where the time goes

From main run 42651976389, seconds (Cargo compilation versus suite execution):

| Suite | macOS compile / execution | Windows compile / execution |
| --- | --- | --- |
| Core, no default features | 59.8 / 572.5 | about 75 / 448.5 |
| Core, streaming | 59.6 / 528.9 | about 77 / 444.7 |
| PocketStation full core suite | not applicable | about 104 / 451.7 |
| Diarization filter | 54.5 / 42.6 | about 96 / 15.5 |
| Desktop unit binary | 144 / 10.1 | already excluded upstream |
| Core integration binary | 30.7 / 43.8 | already excluded upstream |

Core is the slowest test binary. Libtest's completed-test log gaps identify follow-ups, not exact per-test CPU timings: `knowledge::tests::para_normal_updates_recycle_private_generations_beyond_128_then_retract` takes about 60-70 seconds on macOS; `knowledge::tests::source_revision_flip_before_or_after_write_leaves_no_derivative_in_any_adapter` about 71 seconds on Windows; the bounded audio decoder/ffmpeg watcher tests frequently take 15-30 seconds each. These repeat in the full feature suites. No assertion or test implementation is changed here.

## Decision and safety

The split puts the complete no-default and streaming core suites and focused copilot/CLI contracts alongside the existing feature/native/desktop matrix. Both lanes build a real model-free CLI: decoder/watcher fixtures in the full PocketStation suite need a worker binary on the feature runner too. Every suite keeps its existing OS coverage, filters, serial libtest execution and timeout. Both core-only and core-plus-CLI builds remain: the CLI always enables core/streaming, so they check different feature configurations.

Reduce only CI dev/test debug information to `line-tables-only`, retaining source-line backtraces. Debug assertions, overflow checks, optimizations, release profiles and production artifacts keep their existing settings. See [Cargo profile documentation](https://doc.rust-lang.org/cargo/reference/profiles.html#debug). The cache namespace must change for this profile. Both lanes read the same exact-image and toolchain/config-scoped entry, with a lockfile key and a fallback for lockfile edits; only the feature lane on main saves. No additional multi-GB target cache is introduced.

Earlier attempts matter: #1106 restricted saves to main after PR caches thrashed storage; #1109 disabled incremental compilation but did not reduce cache size; #1111 removed the noncritical Linux full-build cache to get below the budget. Keep all three protections and exact ImageOS/ImageVersion scoping. No fallback may restore native objects from a different Xcode/MSVC image.

No platform tests move to Linux. History includes Windows named-pipe behavior, Windows path/atomic-move fixes, macOS socket permissions, #591's desktop test failure, #951/#952's unexecuted whisper/streaming regressions, and the native sherpa/ORT symbol collision (#683/#685). Their execution gates remain intact. `CI Gate` depends on every CI job, including both matrices and the previously omitted Desktop Recall contracts. It rejects failure, cancellation, missing path detection, and skipped required Rust matrices. Required check names remain `CI Gate` and `actionlint`. Tag preflight and release workflows are unchanged. Workflow permissions default to `contents: read`; only path detection additionally gets `pull-requests: read`, required by [paths-filter](https://github.com/dorny/paths-filter#supported-workflows).

Nextest process isolation and sharding could reduce long serial suites, but would change fixture execution semantics. Moving tests between OSes would reduce coverage. sccache would add a second storage system and cannot shorten the measured test execution. A version-only fast path would require a new classifier and separate PR smoke selection from the full main suite. Defer all four until this smaller scheduling/profile change is measured.

## This PR's comparison

[Optimized run 42677647007](https://github.com/silverstein/minutes/actions/runs/42677647007), commit `35f323e8`, passed every CI job. All six Rust jobs had cold caches. Whole-workflow elapsed time was 28.8 minutes, including scheduling and setup; the longest Rust job was 27.8 minutes, versus 56.5 in the recent green main run above. This measures the combined scheduling/profile change, not each optimization independently.

| Optimized job | Wall minutes | Cargo compile/check minutes | Suite execution minutes |
| --- | --- | --- | --- |
| macOS features | 22.7 | 20.7 | 1.0 |
| macOS core | 19.3 | 4.9 | 13.9 |
| Windows features | 27.8 | 19.9 | 6.7 |
| Windows core | 20.8 | 5.9 | 14.2 |
| Linux features | 13.2 | 11.3 | 1.2 |
| Linux core | 6.5 | 3.3 | 2.7 |

All 11 feature-profile inventories report success. The four Windows runnable-test name sets exactly match green main run 42651976389: PocketStation 2,061; streaming 2,054; speech 142; optional engines 160. No test assertions, implementation, platform selection or deadlines changed. The two lanes run in parallel, not one after the other.

The [initial same-PR control](https://github.com/silverstein/minutes/actions/runs/42674095248/attempts/1) started during the [Actions/Checks incident](https://www.githubstatus.com/). GitHub failed to schedule much of its graph; its Windows streaming step remained active beyond its 15-minute deadline and required forced cancellation. Exclude that attempt from clean timing comparisons. The recovery retry reconstructed the missing matrix siblings and passed all three original jobs: macOS 49.8 minutes, Windows 43.1, Linux 22.9. Mac and Windows restored warm caches (3,798 and 1,984 MiB); Linux missed. All 11 runnable and ignored test-name sets exactly match the optimized runs.

Residual risks: more concurrent runners and duplicated CLI fixture builds; reduced debugger variable/type detail in CI; and an initial cold cache. The new namespace is saved only after a main push, so warm reuse and compressed cache size still need verification after a separately approved merge. No warm-cache benefit is included in these results. Nextest/process-level sharding and the pathological fixture timings remain follow-ups.

The final PR verification at [`43dd5ef7`](https://github.com/silverstein/minutes/actions/runs/42682945471) also passed with cold cargo caches. Its feature/core times were macOS 38.7/14.4 minutes, Windows 28.4/19.3, Linux 16.5/7.6. Whole-workflow elapsed time was 43.2 minutes. The Mac compile/link variation between the two optimized runs is material; the 28.8-minute first run is an observation, not a runtime guarantee.

## Post-merge notes

The [first main push run](https://github.com/silverstein/minutes/actions/runs/42691998341) at `567c153e` passed every job. Whole-workflow elapsed time was 43.8 minutes. The slowest Rust job was **Test features (macos-latest), 43.5 minutes**, including its 92-second cache save. PR runs did not measure cache upload time.

| Platform | Features / core minutes | Cache save seconds | Saved cache GiB |
| --- | --- | --- | --- |
| macOS | 43.5 / 21.6 | 92 | 2.89 |
| Windows | 29.5 / 17.5 | 35 | 1.26 |
| Linux | 16.3 / 7.7 | 17 | 1.49 |

All three feature jobs missed on restore, logged successful saves, and had matching new `cargo-lines-v1` entries under `refs/heads/main` in the cache API. The three entries total 5.65 GiB compressed. The repository-wide cache index snapshot was 12.33 GiB, including older namespaces and other workflows; warm reuse and retention under the cache quota remain unverified. No cache entries were deleted.

`core_tests` does not set the macOS `CXXFLAGS` value used by the feature job. Restoring the feature job's cache may therefore cause C++ dependency recompilation in the core job. This cold run cannot verify that warm-restore effect; the flag difference is recorded for later measurement, with no workflow change here.

The `split-core` concurrency namespace and its comment about finishing the original-workflow baseline are leftovers from the timing experiment. The controls have finished. Both remain in the merged workflow; per-PR cancellation semantics are unchanged.
