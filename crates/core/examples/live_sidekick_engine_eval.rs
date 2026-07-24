use std::path::PathBuf;

#[path = "../tests/support/live_sidekick_engine_harness.rs"]
mod harness;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut output_path = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => {
                output_path = Some(PathBuf::from(args.next().ok_or("--out requires a path")?));
            }
            "--json" => {}
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    let report = harness::run_live_sidekick_engine_eval();
    let json = serde_json::to_string_pretty(&report)?;
    if let Some(path) = output_path {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, format!("{json}\n"))?;
        eprintln!("sidekick_engine_eval_artifact={}", path.display());
    }
    println!("{json}");
    if !report.passed {
        std::process::exit(1);
    }
    Ok(())
}
