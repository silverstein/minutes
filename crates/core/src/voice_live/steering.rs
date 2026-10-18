//! Replacement briefs cannot execute until the original bounded job has settled.
use crate::interaction::calls::{CallState, Calls};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Redirects(BTreeMap<String, Value>);

impl Redirects {
    pub fn request(&mut self, calls: &mut Calls, args: &Value) -> Result<Value, String> {
        let id = args["job_id"]
            .as_str()
            .ok_or("Read get_status and supply the exact running prototype job_id")?;
        let brief = args["brief"]
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.len() <= 8000)
            .ok_or("A complete revised brief of at most 8000 bytes is required")?;
        if self.0.len() >= 8 || self.0.contains_key(id) {
            return Err("Redirect already exists or its bounded queue is full.".into());
        }
        let (_, tool, state, _) = calls
            .snapshots()
            .into_iter()
            .find(|(key, _, _, _)| key == id)
            .ok_or("Unknown job")?;
        if !matches!(
            tool.as_str(),
            "build_prototype" | "continue_prototype_redirect"
        ) || !matches!(state, CallState::Queued | CallState::Running)
        {
            return Err(
                "Only a currently queued or running bounded prototype job can be redirected."
                    .into(),
            );
        }
        let agent = args["agent"].as_str().unwrap_or("default");
        if !matches!(agent, "default" | "codex" | "claude") {
            return Err("Unsupported agent".into());
        }
        let mut replacement = json!({"brief":brief,"agent":agent});
        if let Some(previous) = args["previous_id"].as_str() {
            replacement["previous_id"] = json!(previous);
        }
        let state = calls.cancel(id).ok_or("Job disappeared")?;
        self.0.insert(id.into(), replacement);
        Ok(
            json!({"redirect_id":id,"state":format!("{state:?}"),"replacement_started":false,
            "note":"Revised brief stored and original job cancellation requested. Call continue_prototype_redirect with this exact redirect_id; its generation lane waits for the original job to settle. Earlier effects are not undone."}),
        )
    }

    pub fn take(&mut self, calls: &Calls, id: &str) -> Result<Value, String> {
        let state = calls
            .snapshots()
            .into_iter()
            .find(|(key, _, _, _)| key == id)
            .map(|(_, _, s, _)| s)
            .ok_or("Unknown original job")?;
        if !matches!(
            state,
            CallState::Cancelled | CallState::FinishedAfterCancellation
        ) {
            return Err("Original job has not acknowledged cancellation or finished; replacement cannot start.".into());
        }
        self.0
            .remove(id)
            .ok_or("Unknown or already consumed redirect; nothing restarted.".into())
    }
    pub fn clear(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redirect_waits_for_settled_original_and_is_single_use() {
        let mut calls = Calls::default();
        calls.register("original").unwrap();
        calls.name("original", "build_prototype");
        calls.begin("original");
        let mut redirects = Redirects::default();
        redirects
            .request(
                &mut calls,
                &json!({"job_id":"original","brief":"Change direction"}),
            )
            .unwrap();
        assert!(redirects.take(&calls, "original").is_err());
        calls.finish_outcome("original", true, true);
        assert_eq!(
            redirects.take(&calls, "original").unwrap()["brief"],
            "Change direction"
        );
        assert!(redirects.take(&calls, "original").is_err());
    }
    #[test]
    fn broad_agent_jobs_and_discarded_redirects_cannot_execute() {
        let mut calls = Calls::default();
        calls.register("other").unwrap();
        calls.name("other", "ask_agent");
        let mut redirects = Redirects::default();
        assert!(redirects
            .request(
                &mut calls,
                &json!({"job_id":"other","brief":"Not authorized"})
            )
            .is_err());
        calls.name("other", "build_prototype");
        redirects
            .request(
                &mut calls,
                &json!({"job_id":"other","brief":"Local prototype"}),
            )
            .unwrap();
        redirects.clear();
        assert!(redirects.take(&calls, "other").is_err());
    }
}
