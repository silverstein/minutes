# Recording-sidecar worker timings

Issue #967 reports final utterances being dropped under desktop load while the
same sidecar succeeds in isolation. The cause is still unverified. The additive
`worker_timing` object in the existing `live_sidecar_ended` log distinguishes
where the worker spends elapsed time without recording utterance content.

Each stage contains `count`, `total_us`, and `max_us`. A count of zero means that
stage was not observed, not that it completed instantaneously. Durations and
counts saturate rather than wrap. `worker_timing: null` means the worker panicked
and did not return a complete timing summary.

| Stage | Meaning |
| --- | --- |
| `final_queue_wait` | Time from offering a final to the bounded queue until the worker starts dispatching it. Finals discarded at stop are excluded. |
| `whisper_model_load` | Explicit Whisper context-load attempts, including failed attempts. |
| `final_inference` | Final backend calls, excluding the separately measured Whisper model load and transcript write. Backend-internal setup, preprocessing, scheduling delay, and inference remain combined. A Parakeet failure followed by Whisper can produce two attempts for one final. |
| `draft_inference` | Draft backend calls, with the same elapsed-time interpretation. |
| `writer_lock_wait` | Time acquiring the existing transcript-writer lock. |
| `writer_write` | Time inside the existing transcript-write operation. A write attempt is not proof of a durable line; use the existing `lines` and error diagnostics. |

`finals_skipped_at_stop` counts dequeued finals discarded by the existing stop
check. `drafts_taken` counts drafts removed from the latest-only mailbox,
including those whose model load fails. The existing drop and failure counters
keep their meanings. No queue high-water claim is made: the current pending
counter has a separately tracked publication-order race and is unsuitable for
an exact maximum.

The metrics are fixed-size, owned by the inference worker, and serialized only
after its existing join. The audio consumer only attaches an `Instant` to the
already bounded final job. There is no new lock, per-utterance log, audio copy,
provider call, transcript payload, model path, or participant identity in the
timing object. Worker scheduling, model choice, queue capacity, newest-drop
policy, cancellation, and capture/WAV behavior are unchanged.

## Interpreting a desktop reproduction

Use synthetic speech and the signed `Minutes Dev.app` identity. Compare the
same workload and model with the isolation harness, then change screen context
and voice identification one at a time. Compare like-for-like cold and warm
model runs. Queue delay with low backend time suggests work ahead of the final;
long backend time needs CPU/GPU and competing-work evidence; writer delay
points toward lock or storage contention. These are investigation leads, not
causal conclusions. Totals overlap across jobs and must not be added as though
they partition the whole recording. This end summary cannot diagnose a worker
that never returns; it does not add a watchdog or promise crash-time telemetry.

## Decisions retained from history

- `0b7a0622` moved inference off the audio consumer after a real starvation
  incident. Bounded queueing and nonblocking heartbeats remain intact.
- `8fee0209` added capture isolation and stop/WAV preservation invariant tests.
  Diagnostics must satisfy those same invariants.
- `3a38e777` fixed the unsafe whisper-rs abort callback. The raw callback helper
  and its stack-owned lifetime remain unchanged.
- `84bbaa77` deliberately replaced batch elapsed-time cancellation with a
  progress watchdog. This change does not revisit that policy.
- `2f1036f5` persisted sidecar diagnostics because the desktop has no tracing
  subscriber. Timings use the existing persisted summary rather than tracing.
- `6d5f43d9` corrected speech duration to use samples. Existing sample-based
  speech accounting and degraded-status thresholds remain unchanged.
