//! Offline lab. No device access, API credentials or external effects.
use std::io::{self, Read};

use minutes_interaction_contracts::work::WorkCapsule;
use minutes_interaction_contracts::MAX_SNAPSHOT_BYTES;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("new") if args.len() == 3 => {
            println!("{}", WorkCapsule::new(&args[1], &args[2])?.to_json()?);
        }
        Some("resume") if args.len() == 1 => {
            let capsule = WorkCapsule::from_json(&input()?)?;
            println!("{}", capsule.to_markdown()?);
        }
        Some("debrief") if args.len() == 2 => {
            let mut capsule = WorkCapsule::from_json(&input()?)?;
            capsule.debrief_from_host(&args[1], Vec::new(), capsule.revision())?;
            println!("{}", capsule.to_json()?);
        }
        _ => {
            return Err(
                "usage: new ID GOAL | resume < capsule.json | debrief TEXT < capsule.json".into(),
            );
        }
    }
    Ok(())
}

fn input() -> Result<String, String> {
    let mut value = String::new();
    io::stdin()
        .take((MAX_SNAPSHOT_BYTES + 1) as u64)
        .read_to_string(&mut value)
        .map_err(|e| e.to_string())?;
    if value.len() > MAX_SNAPSHOT_BYTES {
        return Err("input exceeds capsule budget".into());
    }
    Ok(value)
}
