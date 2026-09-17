//! Deterministic source document: no coding agent, generated script or guessed URL.
use super::research::Sources;
use crate::{
    config::Config,
    policy_fs::{self, BoundRecoveryDirectory},
};
use serde_json::{json, Value};
use std::path::Path;
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn render(args: &Value, sources: &Sources) -> Result<(String, String), String> {
    let title = args["title"]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 180)
        .ok_or("title must be 1-180 bytes")?;
    let items = args["items"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 12)
        .ok_or("Supply 1-12 source entries")?;
    let mut entries = String::new();
    let mut seen = std::collections::BTreeSet::new();
    for item in items {
        let id = item["source_id"]
            .as_str()
            .ok_or("Each entry needs an exact research source_id")?;
        if !seen.insert(id) {
            return Err("Duplicate reading-list source".into());
        }
        let source = sources.get(id)?;
        let url = source["url"].as_str().ok_or("Missing source URL")?;
        let uri: ureq::http::Uri = url.parse().map_err(|_| "Invalid source URL")?;
        if !matches!(uri.scheme_str(), Some("http" | "https")) || uri.host().is_none() {
            return Err("Unsupported source URL".into());
        }
        let note = item["note"].as_str().unwrap_or("");
        if note.len() > 1200 {
            return Err("Reading note is too long".into());
        }
        let name = source["title"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("Research source");
        entries.push_str(&format!("<li><h2><a href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\">{}</a></h2><p>{}</p><small>Research link; bibliographic details and page availability not independently verified.</small></li>",escape(url),escape(name),escape(note)));
    }
    let html=format!("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'\"><meta name=\"referrer\" content=\"no-referrer\"><title>{0}</title><style>:root{{color-scheme:dark;font:16px ui-monospace,monospace;background:#171a1b;color:#e6eeee;letter-spacing:0}}body{{max-width:860px;margin:0 auto;padding:32px 24px}}h1{{font-size:28px;overflow-wrap:anywhere}}h2{{font-size:18px;line-height:1.4}}ol{{padding-left:26px}}li{{border-top:1px solid #475154;padding:18px 0}}p{{line-height:1.7;white-space:pre-wrap;overflow-wrap:anywhere}}a{{color:#a4ed88;overflow-wrap:anywhere}}a:focus-visible{{outline:2px solid #77cddd}}small{{color:#b0c3c5;font-size:12px}}</style></head><body><h1>{0}</h1><ol>{entries}</ol></body></html>",escape(title));
    Ok((title.to_owned(), html))
}
pub(super) fn create(args: &Value, sources: &Sources) -> Result<Value, String> {
    create_at(
        args,
        sources,
        &Config::minutes_dir().join("reading-lists"),
        true,
    )
}
fn create_at(args: &Value, sources: &Sources, root: &Path, open: bool) -> Result<Value, String> {
    let (title, html) = render(args, sources)?;
    let id = policy_fs::content_sha256_hex(html.as_bytes());
    let dir = BoundRecoveryDirectory::prepare_owner_private(root).map_err(|e| e.to_string())?;
    let name = format!("{id}.html");
    if dir
        .entry_exists(std::ffi::OsStr::new(&name))
        .map_err(|e| e.to_string())?
    {
        let file = dir
            .bind_exact_file(std::ffi::OsStr::new(&name))
            .map_err(|e| e.to_string())?;
        file.recovery_proof_for_exact_bytes_bounded(
            html.as_bytes(),
            64000,
            std::time::Instant::now() + std::time::Duration::from_secs(3),
        )
        .map_err(|e| e.to_string())?;
    } else {
        super::prototype::publish(&dir, &name, html.as_bytes())?;
    }
    let path = root.join(name);
    let opened = open && super::prototype::open_preview(&path);
    Ok(
        json!({"title":title,"path":path,"opened":opened,"saved":true,"coding_agent_used":false,"bibliography_verified":false,"note":"Reading list saved directly from returned sources. Notes are model interpretation, not independently verified bibliography metadata. Opening failure does not lose the saved content."}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reading_list_uses_exact_registered_urls_and_escapes_content() {
        let mut sources = Sources::default();
        let registered=sources.register(json!({"answer":"Fixture","sources":[{"title":"<script>bad</script>","url":"https://publisher.test/article?a=1&b=2"}]}),12000).unwrap();
        let args = json!({"title":"Reading","items":[{"source_id":registered["sources"][0]["source_id"],"note":"<img onerror=alert(1)>"}]});
        let (_, html) = render(&args, &sources).unwrap();
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;img"));
        assert!(html.contains("article?a=1&amp;b=2"));
        assert!(render(
            &json!({"title":"Bad","items":[{"source_id":"invented"}]}),
            &sources
        )
        .is_err());
        let root = tempfile::tempdir().unwrap();
        let result = create_at(&args, &sources, root.path(), false).unwrap();
        assert_eq!(result["coding_agent_used"], false);
        assert_eq!(result["saved"], true);
        create_at(&args, &sources, root.path(), false).unwrap();
    }
}
