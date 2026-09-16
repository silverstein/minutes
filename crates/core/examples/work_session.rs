//! Headless, side-effect-free continuity demonstration. Does not open apps,
//! send messages, access a model, capture a screen, or start a subprocess.
use minutes_core::live_sidekick::work::{
    Focus, Sensitivity, SourceRef, WorkCheckpoint, WorkSession,
};
use std::error::Error;
use std::io::{self, Read};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.as_slice() == ["resume"] {
        // The shell/user owns the file boundary. Input cannot start workers or
        // restore approvals; source metadata must still be freshly attested.
        let mut input = String::new();
        io::stdin()
            .take((minutes_core::live_sidekick::work::MAX_CHECKPOINT_BYTES + 1) as u64)
            .read_to_string(&mut input)?;
        if input.len() > minutes_core::live_sidekick::work::MAX_CHECKPOINT_BYTES {
            return Err("checkpoint exceeds byte budget".into());
        }
        // Text pipes/editors may translate line endings. The codec is canonical LF.
        let input = input.replace("\r\n", "\n");
        let restored = WorkSession::resume(WorkCheckpoint::from_markdown(&input)?)?;
        println!("{}", serde_json::to_string_pretty(&restored.checkpoint())?);
        return Ok(());
    }
    if !args.is_empty() {
        return Err("usage: work_session [resume]; default emits a synthetic checkpoint".into());
    }
    let mut work = WorkSession::new(
        "demo-onboarding".into(),
        "Simplify onboarding without changing pricing".into(),
    )?;
    work.set_focus(Focus {
        source: SourceRef {
            id: "design-v1".into(),
            locator: "minutes://demo/onboarding".into(),
            version: "demo-version-1".into(),
            observed_at_ms: 0,
            sensitivity: Sensitivity::Normal,
        },
        selection: "paragraph:4".into(),
    })?;
    work.add_model_note(
        "The second choice may be unnecessary".into(),
        vec!["design-v1".into()],
    )?;
    work.add_host_note("Keep pricing unchanged".into(), vec![], true)?;
    work.enqueue_task(
        "review-1".into(),
        "Review the two versions; do not edit or send".into(),
    )?;
    let checkpoint = work.park(
        vec!["Does the first review need uploaded documents?".into()],
        Some("Compare the two remaining versions".into()),
    )?;
    print!("{}", checkpoint.to_markdown()?);
    Ok(())
}
