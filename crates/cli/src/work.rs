//! Read-only, portable discovery of saved work for humans and existing agents.
use anyhow::{Context, Result};
use clap::Subcommand;
use minutes_core::live_sidekick::work::{Sensitivity, WorkCheckpoint};
use minutes_core::live_sidekick::work_store::WorkStore;

#[derive(Debug, Subcommand)]
pub enum Action {
    /// List the latest checkpoint for each work session, as JSON.
    List,
    /// Read an exact checkpoint name; never execute its task records.
    Show {
        name: String,
        #[arg(long)]
        markdown: bool,
    },
}

fn safe_for_agent(checkpoint: &WorkCheckpoint) -> bool {
    checkpoint
        .sources
        .iter()
        .all(|s| s.sensitivity == Sensitivity::Normal && s.locator.starts_with("selection:"))
        && checkpoint.tasks.iter().all(|t| t.artifact.is_none())
}

pub fn run(action: Action) -> Result<()> {
    let store = WorkStore::open(&WorkStore::default_path()).map_err(anyhow::Error::msg)?;
    match action {
        Action::List => {
            let mut files = Vec::new();
            for file in store.list().map_err(anyhow::Error::msg)? {
                let checkpoint = store.load(&file.name).map_err(anyhow::Error::msg)?;
                if safe_for_agent(&checkpoint) {
                    files.push(file);
                }
            }
            println!("{}", serde_json::to_string_pretty(&files)?);
        }
        Action::Show { name, markdown } => {
            let checkpoint = store
                .load(&name)
                .map_err(anyhow::Error::msg)
                .context("Could not load checkpoint")?;
            anyhow::ensure!(safe_for_agent(&checkpoint), "This checkpoint includes source-policy or worker-output records that require review in the local Work window. It is not implicitly shareable with an agent.");
            if markdown {
                print!("{}", checkpoint.to_markdown()?);
            } else {
                println!("{}", serde_json::to_string_pretty(&checkpoint)?);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use minutes_core::live_sidekick::work::{SourceRef, WorkSession};
    #[test]
    fn reading_work_does_not_inherit_unverified_source_policy() {
        let mut work = WorkSession::new("test".into(), "Test".into())
            .unwrap()
            .checkpoint();
        assert!(safe_for_agent(&work));
        work.sources.push(SourceRef {
            id: "source".into(),
            locator: "/tmp/secret.md".into(),
            version: "v1".into(),
            observed_at_ms: 0,
            sensitivity: Sensitivity::Restricted,
        });
        assert!(!safe_for_agent(&work));
    }
}
