use minutes_archive_convert::{run_worker_process, WORKER_MARKER};

fn main() {
    let mut arguments = std::env::args();
    let _program = arguments.next();
    let marker = arguments.next();
    let format = arguments.next();
    if marker.as_deref() != Some(WORKER_MARKER) || format.is_none() || arguments.next().is_some() {
        eprintln!("Minutes Archive converter refused an invalid invocation.");
        std::process::exit(64);
    }
    let code = run_worker_process(format.as_deref().unwrap_or_default());
    std::process::exit(code);
}
