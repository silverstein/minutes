# Live input progress diagnostics

Sparse live output can originate before inference. `live_input_progress` entries in the normal structured log sample the optional stem feeder and sidecar consumer independently every ten seconds, plus a final sample when each owner exits. They contain counters, stage labels and wall times, with no audio, transcript text, meeting title or path. They do not establish that a recording is healthy.

Use `pid`, `component` and the process-local `progress_id` to group each component's samples. `progress_id` identifies a diagnostic instance, not a meeting. Compare contemporaneous feeder and consumer instances from the same process; their start times and sample intervals can differ. Counters use 16 kHz mono samples after stem conversion. Snapshot fields are approximate concurrent observations.

| Observation | What it distinguishes |
| --- | --- |
| `voice_samples_read` or `system_samples_read` stops advancing, with polls completing | No new decoded samples were obtained from that stem. This alone does not distinguish writer flushing, file availability or source delivery. |
| Feeder remains in `send`, with growing `stage_age_us` | The existing bounded audio channel is applying backpressure. `samples_sent` advances only after a successful send. |
| Consumer `samples_received` exceeds `samples_processed` | Audio was received but has not completed VAD; `chunk_remaining_samples` includes an in-flight VAD chunk and any unprocessed suffix. |
| Consumer remains in `vad`, `draft_gate`, `heartbeat` or `publish_draft` | The sampled operation has not returned. Inspect its stage age and completed wall-time aggregates separately from inference-worker timings. |
| Consumer remains in `receive` while processed counts are flat | No new chunk has completed receipt. Compare feeder progress before assigning a cause. |

`wall_timing` reports completed operation counts, total microseconds and maximum microseconds. `stage_age_us` describes the current operation, including one that has not returned. All times include scheduling pauses. They are not VAD kernel time and do not prove App Nap. If the whole process is suspended, the sampler can also pause. Missing diagnostics can also mean thread creation or log IO failed.

`live_sidecar_ended` additionally persists exact `samples_fed` and `samples_gated`, plus a consumer snapshot. Queued audio is still subject to the existing stop behavior. Counts alone do not establish speech quality, input completeness or reader use. The diagnostic path never acquires a capture or transcript-writer lock, and input teardown does not wait for diagnostic log IO. This instrumentation does not repair a live delivery stall or change the VAD engine.
