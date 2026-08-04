fn main() {
    let mut arguments = std::env::args();
    let _program = arguments.next();
    let marker = arguments.next();
    let operation = arguments.next();
    let code = match (marker.as_deref(), operation.as_deref(), arguments.next()) {
        (Some(minutes_archive_semantic::WORKER_MARKER), Some(operation), None) => {
            minutes_archive_semantic::run_worker_process(operation)
        }
        _ => 64,
    };
    std::process::exit(code);
}
