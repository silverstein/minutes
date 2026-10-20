# Supervised full speaker worker: integration qualification

This is the follow-up to the [full native comparison](full-native-diarization-2026-10-18.md).
PR #1164 adds an explicitly selected, separately built CPU helper. It does not
change the default model, bundle or download the helper, enable named enrollment,
or replace live transcription. Integration remains in qualification; this document
records passing and failing measurements rather than implying a released feature.

## Frozen controls

The three complete, already-seen public AMI development recordings are IS1008a,
TS3004a and IB4001. They are regression controls, not fresh model admission.
The pinned Speakrs source, model files and defaults match the earlier comparison.
Each recording must preserve its input WAV, return four anonymous speakers, keep
DER within one absolute percentage point of its frozen candidate result and finish
with processing time / recording duration at most 0.25. The worker deadline remains
900 seconds. Build time is outside processing time; host decoding, supervised
launch, model loading and processing are included. No speaker-count hint is supplied.

## Portable build receipts

The compiled integration source for these rows is
`f5e643496d720dfb671edb1c68d222df78f26ff9`.
The [native workflow run](https://github.com/silverstein/minutes/actions/runs/39279757314)
binds the worker, host probe, checker, source files and Linux shared OpenBLAS
library by SHA-256. The reports were recovered from completed job logs and their
source hashes checked against that revision. GitHub's aggregate status lagged the
individual job steps while these receipts were inspected.
The numeric [Linux report](full-speaker-worker-2026-10-19/f5e64349-linux.json) and
[ARM64 report](full-speaker-worker-2026-10-19/f5e64349-arm.json) retain those bindings.
The [failed Intel report](full-speaker-worker-2026-10-19/f5e64349-intel-failed.json)
preserves its source bindings and failed runtime gate too.
The [failed Windows report](full-speaker-worker-2026-10-19/f5e64349-windows-failed.json)
records the 900-second deadline. Its CRLF source hashes were verified against the
same Git revision with checkout line endings applied.

| Platform | IS1008a seconds / ratio | TS3004a seconds / ratio | IB4001 seconds / ratio | Result |
| --- | ---: | ---: | ---: | --- |
| Linux x86-64 | 212.72 / 0.2254 | 299.48 / 0.2226 | 396.84 / 0.2228 | All frozen gates pass |
| macOS ARM64 | 136.32 / 0.1444 | 188.75 / 0.1403 | 253.08 / 0.1421 | All frozen gates pass |
| macOS Intel | 385.06 / 0.4079 | Not run after failure | Not run after failure | Runtime fails |
| Windows x86-64 | Deadline at 900.03 / 0.9535 | Not run after failure | Not run after failure | No completed engine result |

Passing Linux and ARM64 outputs reproduce DER 9.9478%, 21.1965% and 19.9164%,
respectively, with four speakers in each recording and unchanged WAV hashes.
The Intel first recording reproduces DER 9.9478% and four speakers, and preserves
its WAV; those successes do not compensate for its failed runtime gate. Actual
Unix parent-loss retirement also passes on the two fully passing platforms.

Earlier builds failed runtime on Linux (344.09 seconds / 0.3645) and Intel
(477.23 seconds / 0.5056), and the initial Windows attempt exceeded the host's
900-second deadline. Those failures prompted CPU math-library and PCM transport
changes. They were not erased or converted into passes by relaxing the criteria.

## Next measured optimization

The standalone helper is being evaluated with Rust's
[`-C target-cpu=native`](https://doc.rust-lang.org/rustc/codegen-options/index.html#target-cpu).
It must be built on the machine that will run it, and its compiled objects must
not be reused across CPUs. The Minutes host retains its ordinary portable build.
This changes compiler optimization, not model weights, inference parameters,
speaker counts or acceptance limits. It has not yet established an Intel pass.

The [native-CPU Mac replay](full-speaker-worker-2026-10-19/nativecpu-mac.json)
passes all three controls: 45.48, 64.80 and 86.62 seconds, ratios 0.0482, 0.0482
and 0.0486, with the same DER, four speakers and unchanged inputs. Actual parent
loss retirement passes. Release build, strict helper Clippy and all four helper
tests pass with these compiler flags. The report's source and checker hashes
match snapshot `dcac4139d2f4e14653ae420ea38cccb7793a382f`; its compiled Rust source
is unchanged from `f5e64349`.
This was a separate native Mac from the hosted runner; comparing their times does
not measure the compiler flag's speedup. The hosted native-CPU results below qualify each platform separately.

## Hosted native-CPU results

[Run 39288257150](https://github.com/silverstein/minutes/actions/runs/39288257150)
uses source `8115c887fa0b0132e1829fa572c95e341d7ab610`. The numeric
[Linux](full-speaker-worker-2026-10-19/8115c887-linux.json),
[Intel](full-speaker-worker-2026-10-19/8115c887-intel.json),
[ARM64](full-speaker-worker-2026-10-19/8115c887-arm.json) and
[Windows failure](full-speaker-worker-2026-10-19/8115c887-windows-failed.json)
receipts preserve source and executable bindings. Source hashes were verified
against that revision, including Windows checkout line endings.

| Platform | IS1008a seconds / ratio | TS3004a seconds / ratio | IB4001 seconds / ratio | Result |
| --- | ---: | ---: | ---: | --- |
| Linux x86-64 | 266.34 / 0.2822 | Not run after failure | Not run after failure | Runtime fails |
| macOS ARM64 | 149.78 / 0.1587 | 206.74 / 0.1537 | 285.38 / 0.1603 | All frozen gates pass |
| macOS Intel | 382.68 / 0.4054 | Not run after failure | Not run after failure | Runtime fails |
| Windows x86-64 | Failed after 131.55 / 0.1394 | Not run after failure | Not run after failure | No completed engine result |

Completed outputs retain the frozen DER, four speakers and unchanged input hashes.
Linux's earlier portable pass does not qualify this newer head: its measured
runtime fails. These are separate runner executions, not a controlled estimate
of a compiler flag's speedup.

All four platforms pass the actual short-input host/helper smoke. Windows reaches
`pipeline_ready` on the full first recording and then fails. This establishes that
it passed startup, handle fencing, PCM input and pipeline construction. It does
not identify the inference failure or establish a successful engine result.
The ratio in its failure receipt is elapsed time until failure, not processing
throughput. The prior 900-second deadline failure remains in its own receipt.

The next diagnostic adds only a closed pipeline error class and numeric worker
exit status. Error messages, paths, audio and transcript content cannot be
reflected through that channel. Classifier and parser tests reject private
payloads and invalid exit codes; the actual native-helper silence smoke also
passes. No inference parameters, acceptance thresholds, deadline or memory cap
have changed. The new head still needs its own full platform matrix.

Before the full recordings, CI also invokes the actual host/helper on three
seconds of generated silence with a separate 60-second smoke-test budget. That
explicit model test is ignored in ordinary model-free unit runs and executed by
name after the pinned public models are fetched. It checks an empty anonymous
result, no embeddings and byte-for-byte WAV preservation. A failure prevents the
longer qualification from running; it does not substitute for those recordings.

## Local stage diagnostic check

The [one-control Linux receipt](full-speaker-worker-2026-10-19/stage-profile-linux.json)
binds the diagnostic source `3cf92ce619cc0396af8ea8dc3395ce63ad998519`, executable
hashes and input. IS1008a completed in 189.49 seconds (ratio 0.2007), preserving
the WAV, four speakers and DER 9.9478%. Cumulative inference timings were 23.272
seconds at segmentation completion, 171.443 at embedding completion and 171.532
at inference completion. The interval between segmentation and embedding
completion was 148.171 seconds; it includes decoding, preparation and embedding.
The remaining inference interval was 0.089 seconds. These stage boundaries
identify a place to profile rather than measure individual kernels or establish
a faster implementation.

Whole-workspace Clippy was running concurrently with two Cargo jobs. This is
one already-seen diagnostic control, not an isolated throughput measurement,
complete platform qualification or evidence about the Windows crash. The actual
three-second silence test also passed with all three timing fields present.
Strict helper and feature-specific host Clippy, seven helper tests, ten host
tests, twenty Python controls (two optional skips), sixteen coverage-auditor
tests, actionlint and generated-doc checks passed. Whole-workspace Clippy passed
on macOS with identical source bytes. The Linux whole-workspace attempt reported
unused-code lint errors in the unchanged Tauri platform code; the changed core
feature and helper passed their separate strict Linux checks.

## Scope still unqualified

The next diagnostic revision observes the pinned pipeline's existing
`Segmentation complete` and `Embeddings complete` events through a bounded
subscriber. It emits only fixed labels and cumulative elapsed milliseconds
from inference start; the host and receipt parser accept only those labels and
numbers within the existing 900-second envelope. Successful recordings retain
the timings too. This should distinguish segmentation from embedding and
post-processing cost without changing the model or inference path. It does not
yet establish the Windows crash's cause or a speed improvement. The Windows
workflow also saves a successfully built pinned static BLAS library before any
later inference failure, preserving its existing image/CPU/triplet/source key.

Anonymous-label protocol checks, cancellation and process-tree retirement are
separate from end-to-end meeting acceptance. Real ASR word alignment and overlap,
multilingual and Dutch conversations, short turns, similar voices, headphone calls,
named false positives, concurrent live-capture latency and signed installed-app
parity remain open. The original benchmark's oracle-word scores are not ASR
accuracy measurements. No universal SOTA, production-default or installed-app
acceptance claim follows from these regression recordings.
