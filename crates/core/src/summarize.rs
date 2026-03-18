use crate::config::Config;

// ──────────────────────────────────────────────────────────────
// LLM summarization module (pluggable).
//
// Supported engines:
//   "claude"  → Anthropic Claude API (ANTHROPIC_API_KEY env var)
//   "openai"  → OpenAI API (OPENAI_API_KEY env var)
//   "ollama"  → Local Ollama server (no API key needed)
//   "none"    → Skip summarization (default)
//
// For long transcripts: map-reduce chunking.
//   Chunk by time segments → summarize each chunk → synthesize final.
// ──────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct Summary {
    pub text: String,
    pub decisions: Vec<String>,
    pub action_items: Vec<String>,
    pub key_points: Vec<String>,
}

/// Summarize a transcript using the configured LLM engine.
/// Returns None if summarization is disabled or fails gracefully.
pub fn summarize(transcript: &str, config: &Config) -> Option<Summary> {
    let engine = &config.summarization.engine;

    if engine == "none" {
        return None;
    }

    tracing::info!(engine = %engine, "running LLM summarization");

    let result = match engine.as_str() {
        "claude" => summarize_with_claude(transcript, config),
        "openai" => summarize_with_openai(transcript, config),
        "ollama" => summarize_with_ollama(transcript, config),
        other => {
            tracing::warn!(engine = %other, "unknown summarization engine, skipping");
            return None;
        }
    };

    match result {
        Ok(summary) => {
            tracing::info!(
                decisions = summary.decisions.len(),
                action_items = summary.action_items.len(),
                key_points = summary.key_points.len(),
                "summarization complete"
            );
            Some(summary)
        }
        Err(e) => {
            tracing::error!(error = %e, "summarization failed, continuing without summary");
            None
        }
    }
}

/// Format a Summary into markdown sections.
pub fn format_summary(summary: &Summary) -> String {
    let mut output = String::new();

    if !summary.key_points.is_empty() {
        for point in &summary.key_points {
            output.push_str(&format!("- {}\n", point));
        }
    } else if !summary.text.is_empty() {
        output.push_str(&summary.text);
        output.push('\n');
    }

    if !summary.decisions.is_empty() {
        output.push_str("\n## Decisions\n\n");
        for decision in &summary.decisions {
            output.push_str(&format!("- [x] {}\n", decision));
        }
    }

    if !summary.action_items.is_empty() {
        output.push_str("\n## Action Items\n\n");
        for item in &summary.action_items {
            output.push_str(&format!("- [ ] {}\n", item));
        }
    }

    output
}

// ── Prompt ────────────────────────────────────────────────────

const SYSTEM_PROMPT: &str = r#"You are a meeting summarizer. Given a transcript, extract:
1. Key points (3-5 bullet points summarizing what was discussed)
2. Decisions (any decisions that were made)
3. Action items (tasks assigned to specific people, with deadlines if mentioned)

Respond in this exact format:

KEY POINTS:
- point 1
- point 2

DECISIONS:
- decision 1

ACTION ITEMS:
- @person: task description (by deadline if mentioned)"#;

fn build_prompt(transcript: &str, chunk_max_tokens: usize) -> Vec<String> {
    // Rough token estimate: ~4 chars per token
    let max_chars = chunk_max_tokens * 4;

    if transcript.len() <= max_chars {
        return vec![transcript.to_string()];
    }

    // Split into chunks at line boundaries
    let mut chunks = Vec::new();
    let mut current = String::new();

    for line in transcript.lines() {
        if current.len() + line.len() > max_chars && !current.is_empty() {
            chunks.push(current.clone());
            current.clear();
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

fn parse_summary_response(response: &str) -> Summary {
    let mut key_points = Vec::new();
    let mut decisions = Vec::new();
    let mut action_items = Vec::new();
    let mut current_section = "";

    for line in response.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("KEY POINTS:") {
            current_section = "key_points";
            continue;
        } else if trimmed.starts_with("DECISIONS:") {
            current_section = "decisions";
            continue;
        } else if trimmed.starts_with("ACTION ITEMS:") {
            current_section = "action_items";
            continue;
        }

        if let Some(item) = trimmed.strip_prefix("- ") {
            match current_section {
                "key_points" => key_points.push(item.to_string()),
                "decisions" => decisions.push(item.to_string()),
                "action_items" => action_items.push(item.to_string()),
                _ => {}
            }
        }
    }

    Summary {
        text: if key_points.is_empty() {
            response.to_string()
        } else {
            String::new()
        },
        decisions,
        action_items,
        key_points,
    }
}

// ── Claude API ───────────────────────────────────────────────

fn summarize_with_claude(
    transcript: &str,
    config: &Config,
) -> Result<Summary, Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "ANTHROPIC_API_KEY not set. Export it or switch to engine = \"ollama\"")?;

    let chunks = build_prompt(transcript, config.summarization.chunk_max_tokens);
    let mut all_summaries = Vec::new();

    for (i, chunk) in chunks.iter().enumerate() {
        if chunks.len() > 1 {
            tracing::info!(chunk = i + 1, total = chunks.len(), "summarizing chunk");
        }

        let body = serde_json::json!({
            "model": "claude-sonnet-4-20250514",
            "max_tokens": 1024,
            "system": SYSTEM_PROMPT,
            "messages": [{
                "role": "user",
                "content": format!("Summarize this transcript:\n\n{}", chunk)
            }]
        });

        let response = http_post(
            "https://api.anthropic.com/v1/messages",
            &body,
            &[
                ("x-api-key", &api_key),
                ("anthropic-version", "2023-06-01"),
                ("content-type", "application/json"),
            ],
        )?;

        let text = extract_claude_text(&response)?;
        all_summaries.push(text);
    }

    // If multiple chunks, do a final synthesis
    let final_text = if all_summaries.len() > 1 {
        let combined = all_summaries.join("\n\n---\n\n");
        let synth_body = serde_json::json!({
            "model": "claude-sonnet-4-20250514",
            "max_tokens": 1024,
            "system": "Combine these partial meeting summaries into a single cohesive summary. Use the same KEY POINTS / DECISIONS / ACTION ITEMS format.",
            "messages": [{
                "role": "user",
                "content": format!("Combine these summaries:\n\n{}", combined)
            }]
        });

        let response = http_post(
            "https://api.anthropic.com/v1/messages",
            &synth_body,
            &[
                ("x-api-key", &api_key),
                ("anthropic-version", "2023-06-01"),
                ("content-type", "application/json"),
            ],
        )?;
        extract_claude_text(&response)?
    } else {
        all_summaries.into_iter().next().unwrap_or_default()
    };

    Ok(parse_summary_response(&final_text))
}

fn extract_claude_text(response: &serde_json::Value) -> Result<String, Box<dyn std::error::Error>> {
    response["content"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|block| block["text"].as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("unexpected Claude API response: {}", response).into())
}

// ── OpenAI API ───────────────────────────────────────────────

fn summarize_with_openai(
    transcript: &str,
    config: &Config,
) -> Result<Summary, Box<dyn std::error::Error>> {
    let api_key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| "OPENAI_API_KEY not set. Export it or switch to engine = \"ollama\"")?;

    let chunks = build_prompt(transcript, config.summarization.chunk_max_tokens);
    let mut all_text = String::new();

    for chunk in &chunks {
        let body = serde_json::json!({
            "model": "gpt-4o-mini",
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": format!("Summarize this transcript:\n\n{}", chunk) }
            ],
            "max_tokens": 1024,
        });

        let response = http_post(
            "https://api.openai.com/v1/chat/completions",
            &body,
            &[
                ("Authorization", &format!("Bearer {}", api_key)),
                ("Content-Type", "application/json"),
            ],
        )?;

        let text = response["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();
        all_text.push_str(&text);
        all_text.push('\n');
    }

    Ok(parse_summary_response(&all_text))
}

// ── Ollama (local) ───────────────────────────────────────────

fn summarize_with_ollama(
    transcript: &str,
    config: &Config,
) -> Result<Summary, Box<dyn std::error::Error>> {
    let chunks = build_prompt(transcript, config.summarization.chunk_max_tokens);
    let mut all_text = String::new();

    for chunk in &chunks {
        let body = serde_json::json!({
            "model": &config.summarization.ollama_model,
            "prompt": format!("{}\n\nSummarize this transcript:\n\n{}", SYSTEM_PROMPT, chunk),
            "stream": false,
        });

        let url = format!("{}/api/generate", config.summarization.ollama_url);
        let response = http_post(&url, &body, &[("Content-Type", "application/json")])?;

        let text = response["response"].as_str().unwrap_or("").to_string();
        all_text.push_str(&text);
        all_text.push('\n');
    }

    Ok(parse_summary_response(&all_text))
}

// ── HTTP helper (ureq — pure Rust, no subprocess, no secrets in process args) ──

fn http_post(
    url: &str,
    body: &serde_json::Value,
    headers: &[(&str, &str)],
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let mut request = ureq::post(url);

    for (key, value) in headers {
        request = request.header(*key, *value);
    }

    let response: serde_json::Value = request.send_json(body)?.body_mut().read_json()?;

    // Check for API errors
    if let Some(error) = response.get("error") {
        return Err(format!("API error: {}", error).into());
    }

    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_summary_response_extracts_sections() {
        let response = "\
KEY POINTS:
- Discussed pricing strategy
- Agreed on annual billing/month minimum

DECISIONS:
- Price advisor platform at annual billing/mo

ACTION ITEMS:
- @user: Send pricing doc by Friday
- @case: Review competitor grid";

        let summary = parse_summary_response(response);
        assert_eq!(summary.key_points.len(), 2);
        assert_eq!(summary.decisions.len(), 1);
        assert_eq!(summary.action_items.len(), 2);
        assert!(summary.action_items[0].contains("@mat"));
    }

    #[test]
    fn parse_summary_response_handles_freeform_text() {
        let response = "This meeting covered pricing and roadmap. No specific decisions.";
        let summary = parse_summary_response(response);
        assert!(summary.key_points.is_empty());
        assert!(!summary.text.is_empty());
    }

    #[test]
    fn build_prompt_returns_single_chunk_for_short_transcript() {
        let transcript = "Short transcript.";
        let chunks = build_prompt(transcript, 4000);
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn build_prompt_splits_long_transcript() {
        // Create a transcript longer than 100 chars (chunk_max_tokens=25 → 100 chars)
        let transcript = (0..20)
            .map(|i| {
                format!(
                    "[0:{:02}] This is line number {} of the transcript.\n",
                    i, i
                )
            })
            .collect::<String>();
        let chunks = build_prompt(&transcript, 25);
        assert!(chunks.len() > 1, "should split into multiple chunks");
    }

    #[test]
    fn format_summary_produces_markdown() {
        let summary = Summary {
            text: String::new(),
            key_points: vec!["Point one".into(), "Point two".into()],
            decisions: vec!["Decision A".into()],
            action_items: vec!["@user: Do the thing".into()],
        };
        let md = format_summary(&summary);
        assert!(md.contains("- Point one"));
        assert!(md.contains("## Decisions"));
        assert!(md.contains("- [x] Decision A"));
        assert!(md.contains("## Action Items"));
        assert!(md.contains("- [ ] @user: Do the thing"));
    }

    #[test]
    fn summarize_returns_none_when_disabled() {
        let config = Config::default(); // engine = "none"
        let result = summarize("some transcript", &config);
        assert!(result.is_none());
    }
}
