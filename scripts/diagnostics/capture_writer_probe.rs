// Temporary synthetic-fixture observer, inserted only in a disposable test checkout.
mod capture_writer_probe {
    use std::io::{Seek, SeekFrom, Write};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    struct Probe {
        started: Instant,
        state: Mutex<serde_json::Value>,
    }

    static ACTIVE: Mutex<Option<Arc<Probe>>> = Mutex::new(None);

    pub(super) struct ObservedFile {
        file: std::fs::File,
        probe: Option<Arc<Probe>>,
    }

    impl ObservedFile {
        pub(super) fn new(file: std::fs::File, path: &std::path::Path) -> Self {
            let probe = (path.file_name() == Some(std::ffi::OsStr::new("tail.wav"))).then(|| {
                let probe = Arc::new(Probe {
                    started: Instant::now(),
                    state: Mutex::new(serde_json::json!({
                        "operations": [], "stages": [], "worker_finished": false,
                        "file_write_calls": 0, "file_bytes_written": 0,
                        "file_operation_wall_us": 0, "max_file_operation_us": 0,
                        "active_operation": null
                    })),
                });
                *ACTIVE.lock().unwrap() = Some(Arc::clone(&probe));
                probe
            });
            Self { file, probe }
        }

        fn begin(&self, operation: &str, requested_bytes: usize) -> Instant {
            let started = Instant::now();
            if let Some(probe) = &self.probe {
                probe.state.lock().unwrap()["active_operation"] = serde_json::json!({
                    "operation": operation, "requested_bytes": requested_bytes,
                    "started_us": probe.started.elapsed().as_micros()
                });
            }
            started
        }

        fn end<T>(
            &self,
            operation: &str,
            started: Instant,
            bytes: usize,
            result: &std::io::Result<T>,
        ) {
            if let Some(probe) = &self.probe {
                let elapsed = started.elapsed().as_micros() as u64;
                let mut state = probe.state.lock().unwrap();
                state["file_operation_wall_us"] =
                    (state["file_operation_wall_us"].as_u64().unwrap() + elapsed).into();
                state["max_file_operation_us"] = elapsed
                    .max(state["max_file_operation_us"].as_u64().unwrap())
                    .into();
                if operation == "write" {
                    state["file_write_calls"] =
                        (state["file_write_calls"].as_u64().unwrap() + 1).into();
                    state["file_bytes_written"] =
                        (state["file_bytes_written"].as_u64().unwrap() + bytes as u64).into();
                }
                let event = serde_json::json!({"operation": operation, "wall_us": elapsed,
                    "bytes": bytes, "error_kind": result.as_ref().err().map(|e| format!("{:?}", e.kind()))});
                let events = state["operations"].as_array_mut().unwrap();
                if events.len() < 64 {
                    events.push(event);
                }
                state["active_operation"] = serde_json::Value::Null;
            }
        }
    }

    impl Write for ObservedFile {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let started = self.begin("write", bytes.len());
            let result = self.file.write(bytes);
            self.end("write", started, *result.as_ref().unwrap_or(&0), &result);
            result
        }
        fn flush(&mut self) -> std::io::Result<()> {
            let started = self.begin("flush", 0);
            let result = self.file.flush();
            self.end("flush", started, 0, &result);
            result
        }
    }

    impl Seek for ObservedFile {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            let started = self.begin("seek", 0);
            let result = self.file.seek(position);
            self.end("seek", started, 0, &result);
            result
        }
    }

    pub(super) fn stage(name: &str, accepted: u64, queued: usize, spawn_delay_us: Option<u128>) {
        if let Some(probe) = ACTIVE.lock().unwrap().as_ref() {
            let mut state = probe.state.lock().unwrap();
            let event = serde_json::json!({"phase": name, "at_us": probe.started.elapsed().as_micros(),
                "accepted_samples": accepted, "queued_samples": queued, "spawn_delay_us": spawn_delay_us});
            let stages = state["stages"].as_array_mut().unwrap();
            if stages.len() < 64 {
                stages.push(event);
            }
            if name == "worker_finished" {
                state["worker_finished"] = true.into();
            }
        }
    }

    pub(super) fn worker_finished() -> bool {
        ACTIVE
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|p| p.state.lock().unwrap()["worker_finished"] == true)
    }

    pub(super) fn report(
        phase: &str,
        stop_wall_us: u128,
        result: &Result<(), super::CaptureError>,
        accepted: u64,
        queued: usize,
        failed: bool,
    ) {
        if let Some(probe) = ACTIVE.lock().unwrap().as_ref() {
            let result_kind = match result {
                Ok(()) => "ok",
                Err(error) if error.storage_is_retiring() => "storage_retiring",
                Err(_) => "other_error",
            };
            let mut state = probe.state.lock().unwrap().clone();
            state["phase"] = phase.into();
            state["stop_wall_us"] = serde_json::json!(stop_wall_us);
            state["elapsed_us"] = serde_json::json!(probe.started.elapsed().as_micros());
            state["stop_result"] = result_kind.into();
            state["accepted_samples"] = accepted.into();
            state["queued_samples"] = queued.into();
            state["storage_failed"] = failed.into();
            eprintln!("CAPTURE_WRITER_PROBE {state}");
            if let Some(path) = std::env::var_os("MINUTES_CAPTURE_WRITER_PROBE_RESULT") {
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    let _ = writeln!(file, "{state}");
                }
            }
        }
    }

    pub(super) fn clear() {
        *ACTIVE.lock().unwrap() = None;
    }
}
