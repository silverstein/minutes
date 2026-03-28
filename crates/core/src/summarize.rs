use crate::config::Config;

// ──────────────────────────────────────────────────────────────
// LLM summarization module (pluggable).
//
// Supported engines:
//   "none"    → Skip summarization — Claude summarizes via MCP when asked (default)
//   "agent"   → Agent CLI (claude -p, codex exec) — uses existing subscription, no API key
//   "ollama"  → Local Ollama server (no API key needed)
//   "claude"  → Anthropic Claude API (ANTHROPIC_API_KEY env var, legacy)
//   "openai"  → OpenAI API (OPENAI_API_KEY env var, legacy)
//   "mistral" → Mistral API (MISTRAL_API_KEY env var)
//
// For long transcripts: map-reduce chunking.
//   Chunk by time segments → summarize each chunk → synthesize final.
// ──────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct Summary {
    pub text: String,
    pub decisions: Vec<String>,
    pub action_items: Vec<String>,
    pub open_questions: Vec<String>,
    pub commitments: Vec<String>,
    pub key_points: Vec<String>,
    pub participants: Vec<String>,
}

/// Summarize a transcript using the configured LLM engine.
/// Optionally includes screen context images for vision-capable models.
/// Returns None if summarization is disabled or fails gracefully.
pub fn summarize(transcript: &str, config: &Config) -> Option<Summary> {
    summarize_with_screens(transcript, &[], config)
}

/// Summarize a transcript with optional screen context screenshots.
/// Screen images are sent as base64-encoded image content to vision-capable LLMs.
pub fn summarize_with_screens(
    transcript: &str,
    screen_files: &[std::path::PathBuf],
    config: &Config,
) -> Option<Summary> {
    let engine = &config.summarization.engine;

    if engine == "none" {
        return None;
    }

    tracing::info!(engine = %engine, "running LLM summarization");

    let result = match engine.as_str() {
        "agent" => summarize_with_agent(transcript, config),
        "claude" => summarize_with_claude(transcript, screen_files, config),
        "openai" => summarize_with_openai(transcript, screen_files, config),
        "mistral" => summarize_with_mistral(transcript, screen_files, config),
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
                open_questions = summary.open_questions.len(),
                commitments = summary.commitments.len(),
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

    if !summary.open_questions.is_empty() {
        output.push_str("\n## Open Questions\n\n");
        for question in &summary.open_questions {
            output.push_str(&format!("- {}\n", question));
        }
    }

    if !summary.commitments.is_empty() {
        output.push_str("\n## Commitments\n\n");
        for commitment in &summary.commitments {
            output.push_str(&format!("- {}\n", commitment));
        }
    }

    output
}

// ── Prompt ────────────────────────────────────────────────────

const SYSTEM_PROMPT: &str = r#"You are a meeting summarizer. Given a transcript, extract:
1. Key points (3-5 bullet points summarizing what was discussed)
2. Decisions (any decisions that were made)
3. Action items (tasks assigned to specific people, with deadlines if mentioned)
4. Open questions (unresolved questions or unknowns that still need follow-up)
5. Commitments (explicit promises, commitments, or owner statements made by someone)
6. Participants (names of people present or mentioned in the conversation)

Respond in this exact format:

KEY POINTS:
- point 1
- point 2

DECISIONS:
- decision 1

ACTION ITEMS:
- @person: task description (by deadline if mentioned)

OPEN QUESTIONS:
- question 1

COMMITMENTS:
- @person: commitment description (by deadline if mentioned)

PARTICIPANTS:
- Name (role if mentioned)"#;

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
    let mut open_questions = Vec::new();
    let mut commitments = Vec::new();
    let mut participants_raw = Vec::new();
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
        } else if trimmed.starts_with("OPEN QUESTIONS:") {
            current_section = "open_questions";
            continue;
        } else if trimmed.starts_with("COMMITMENTS:") {
            current_section = "commitments";
            continue;
        } else if trimmed.starts_with("PARTICIPANTS:") {
            current_section = "participants";
            continue;
        }

        if let Some(item) = trimmed.strip_prefix("- ") {
            match current_section {
                "key_points" => key_points.push(item.to_string()),
                "decisions" => decisions.push(item.to_string()),
                "action_items" => action_items.push(item.to_string()),
                "open_questions" => open_questions.push(item.to_string()),
                "commitments" => commitments.push(item.to_string()),
                "participants" => participants_raw.push(item.to_string()),
                _ => {}
            }
        }
    }

    // Strip role annotations: "Dan (patent attorney)" → "Dan"
    let participants = participants_raw
        .into_iter()
        .map(|p| {
            if let Some(paren) = p.find(" (") {
                p[..paren].trim().to_string()
            } else {
                p.trim().to_string()
            }
        })
        .filter(|p| !p.is_empty())
        .collect();

    Summary {
        text: if key_points.is_empty() {
            response.to_string()
        } else {
            String::new()
        },
        decisions,
        action_items,
        open_questions,
        commitments,
        key_points,
        participants,
    }
}

// ── Agent CLI (claude -p, codex exec, etc.) ─────────────────
//
// Uses the user's installed AI agent CLI to summarize.
// No API keys needed — uses the agent's own auth (subscription, OAuth, etc.)
//
// Supported agents:
//   "claude" → `claude -p "prompt" --no-input` (Claude Code CLI)
//   "codex"  → `codex exec "prompt"` (OpenAI Codex CLI)
//   Any other → treated as a command that accepts a prompt on stdin
//
// The agent command is configurable via [summarization] agent_command.

/// Resolve a command name to a full path, searching common install locations.
/// GUI apps (like Tauri) run with a minimal PATH that doesn't include
/// ~/.cargo/bin, ~/.local/bin, or /opt/homebrew/bin.
fn resolve_agent_path(cmd: &str) -> String {
    use std::path::PathBuf;

    // Already an absolute path
    if cmd.starts_with('/') {
        return cmd.to_string();
    }

    // Check if it's findable in the current PATH
    if let Ok(output) = std::process::Command::new("which").arg(cmd).output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return path;
            }
        }
    }

    // Search common install directories
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    let search_dirs = [
        home.join(".cargo/bin"),
        home.join(".local/bin"),
        home.join(".npm-global/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
    ];

    for dir in &search_dirs {
        let candidate = dir.join(cmd);
        if candidate.exists() {
            return candidate.to_string_lossy().to_string();
        }
    }

    // Fall back to bare command name (will likely fail in GUI context)
    cmd.to_string()
}

fn summarize_with_agent(
    transcript: &str,
    config: &Config,
) -> Result<Summary, Box<dyn std::error::Error>> {
    use std::io::Write;

    let agent_cmd = if config.summarization.agent_command.is_empty() {
        "claude".to_string()
    } else {
        config.summarization.agent_command.clone()
    };

    // Resolve full path — GUI apps have a minimal PATH and won't find
    // binaries in ~/.cargo/bin, ~/.local/bin, /opt/homebrew/bin, etc.
    let agent_cmd = resolve_agent_path(&agent_cmd);

    // Truncate at a safe UTF-8 char boundary to avoid panics
    let max_transcript = 100_000;
    let truncated = if transcript.len() > max_transcript {
        let mut end = max_transcript;
        while end > 0 && !transcript.is_char_boundary(end) {
            end -= 1;
        }
        &transcript[..end]
    } else {
        transcript
    };

    let prompt = format!(
        "{}\n\nSummarize this transcript:\n\n{}",
        SYSTEM_PROMPT, truncated
    );

    tracing::info!(agent = %agent_cmd, prompt_len = prompt.len(), "summarizing via agent CLI");

    // All agents use stdin/pipe to avoid OS ARG_MAX limits.
    // A 100K transcript as a CLI argument works on most systems but is fragile.
    // Piping is universally safe and works with all agents.
    let (cmd, args): (&str, Vec<&str>) = if agent_cmd == "claude" || agent_cmd.ends_with("/claude")
    {
        (&agent_cmd, vec!["-p", "-", "--no-input"])
    } else if agent_cmd == "codex" || agent_cmd.ends_with("/codex") {
        (&agent_cmd, vec!["exec", "-", "-s", "read-only"])
    } else {
        (&agent_cmd, vec![])
    };

    let mut child = std::process::Command::new(cmd)
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!(
                "Agent '{}' not found or failed to start: {}. \
                 Install it or change [summarization] agent_command in config.toml",
                agent_cmd, e
            )
        })?;

    // Write prompt to stdin
    if let Some(mut stdin) = child.stdin.take() {
        // Write in a thread to avoid deadlock if the process buffer fills
        let prompt_bytes = prompt.into_bytes();
        std::thread::spawn(move || {
            stdin.write_all(&prompt_bytes).ok();
            // stdin drops here, closing the pipe
        });
    }

    // Wait with a 5-minute timeout (long meetings = long summaries)
    let timeout = std::time::Duration::from_secs(300);
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child
                    .wait_with_output()
                    .map_err(|e| format!("Failed to read agent output: {}", e))?;

                if !status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(
                        format!("Agent '{}' exited with error: {}", agent_cmd, stderr).into(),
                    );
                }

                let response = String::from_utf8_lossy(&output.stdout).to_string();
                if response.trim().is_empty() {
                    return Err(format!("Agent '{}' returned empty output", agent_cmd).into());
                }

                tracing::info!(
                    agent = %agent_cmd,
                    response_len = response.len(),
                    "agent summarization complete"
                );

                return Ok(parse_summary_response(&response));
            }
            Ok(None) => {
                // Still running
                if start.elapsed() > timeout {
                    child.kill().ok();
                    return Err(format!(
                        "Agent '{}' timed out after {}s",
                        agent_cmd,
                        timeout.as_secs()
                    )
                    .into());
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            Err(e) => {
                return Err(format!("Failed to check agent status: {}", e).into());
            }
        }
    }
}

// ── Claude API ───────────────────────────────────────────────

fn summarize_with_claude(
    transcript: &str,
    screen_files: &[std::path::PathBuf],
    config: &Config,
) -> Result<Summary, Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "ANTHROPIC_API_KEY not set. Export it or switch to engine = \"ollama\"")?;

    let chunks = build_prompt(transcript, config.summarization.chunk_max_tokens);
    let mut all_summaries = Vec::new();

    // Encode screen context images as base64 for the first chunk only
    let screen_content = encode_screens_for_claude(screen_files);

    for (i, chunk) in chunks.iter().enumerate() {
        if chunks.len() > 1 {
            tracing::info!(chunk = i + 1, total = chunks.len(), "summarizing chunk");
        }

        // Build multimodal content: images (first chunk only) + text
        let mut content_blocks: Vec<serde_json::Value> = Vec::new();

        // Include screen context images in the first chunk
        if i == 0 && !screen_content.is_empty() {
            tracing::info!(
                images = screen_content.len(),
                "sending screen context to Claude"
            );
            content_blocks.extend(screen_content.clone());
            content_blocks.push(serde_json::json!({
                "type": "text",
                "text": "The images above show what was on screen during this meeting. Use them for context when speakers reference visual content.\n\n"
            }));
        }

        content_blocks.push(serde_json::json!({
            "type": "text",
            "text": format!("Summarize this transcript:\n\n{}", chunk)
        }));

        let body = serde_json::json!({
            "model": "claude-sonnet-4-20250514",
            "max_tokens": 1024,
            "system": SYSTEM_PROMPT,
            "messages": [{
                "role": "user",
                "content": content_blocks
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

/// Extract text from an OpenAI-compatible chat completion response.
/// Used by OpenAI and Mistral engines (both use the same response shape).
fn extract_chat_completion_text(
    response: &serde_json::Value,
    engine: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    response["choices"]
        .get(0)
        .and_then(|choice| choice["message"]["content"].as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("unexpected {} API response: {}", engine, response).into())
}

// ── OpenAI API ───────────────────────────────────────────────

fn summarize_with_openai(
    transcript: &str,
    screen_files: &[std::path::PathBuf],
    config: &Config,
) -> Result<Summary, Box<dyn std::error::Error>> {
    let api_key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| "OPENAI_API_KEY not set. Export it or switch to engine = \"ollama\"")?;

    let chunks = build_prompt(transcript, config.summarization.chunk_max_tokens);
    let mut all_text = String::new();

    let screen_content = encode_screens_for_openai(screen_files);

    for (i, chunk) in chunks.iter().enumerate() {
        // Build multimodal content for OpenAI
        let mut content_parts: Vec<serde_json::Value> = Vec::new();

        if i == 0 && !screen_content.is_empty() {
            tracing::info!(
                images = screen_content.len(),
                "sending screen context to OpenAI"
            );
            content_parts.extend(screen_content.clone());
            content_parts.push(serde_json::json!({
                "type": "text",
                "text": "The images above show what was on screen during this meeting. Use them for context.\n\n"
            }));
        }

        content_parts.push(serde_json::json!({
            "type": "text",
            "text": format!("Summarize this transcript:\n\n{}", chunk)
        }));

        // Use gpt-4o (vision-capable) when we have images, gpt-4o-mini otherwise
        let model = if i == 0 && !screen_content.is_empty() {
            "gpt-4o"
        } else {
            "gpt-4o-mini"
        };

        let body = serde_json::json!({
            "model": model,
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": content_parts }
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

        let text = extract_chat_completion_text(&response, "OpenAI")?;
        all_text.push_str(&text);
        all_text.push('\n');
    }

    Ok(parse_summary_response(&all_text))
}

// ── Mistral API ─────────────────────────────────────────────

fn summarize_with_mistral(
    transcript: &str,
    screen_files: &[std::path::PathBuf],
    config: &Config,
) -> Result<Summary, Box<dyn std::error::Error>> {
    let api_key = std::env::var("MISTRAL_API_KEY")
        .map_err(|_| "MISTRAL_API_KEY not set. Export it or switch to engine = \"ollama\"")?;

    let model = &config.summarization.mistral_model;
    let chunks = build_prompt(transcript, config.summarization.chunk_max_tokens);
    let mut all_summaries = Vec::new();

    let screen_content = encode_screens_for_mistral(screen_files);

    for (i, chunk) in chunks.iter().enumerate() {
        if chunks.len() > 1 {
            tracing::info!(chunk = i + 1, total = chunks.len(), "summarizing chunk");
        }

        let mut content_parts: Vec<serde_json::Value> = Vec::new();

        if i == 0 && !screen_content.is_empty() {
            tracing::info!(
                images = screen_content.len(),
                "sending screen context to Mistral"
            );
            content_parts.extend(screen_content.clone());
            content_parts.push(serde_json::json!({
                "type": "text",
                "text": "The images above show what was on screen during this meeting. Use them for context.\n\n"
            }));
        }

        content_parts.push(serde_json::json!({
            "type": "text",
            "text": format!("Summarize this transcript:\n\n{}", chunk)
        }));

        let body = serde_json::json!({
            "model": model,
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": content_parts }
            ],
            "max_tokens": 1024,
        });

        let response = http_post(
            "https://api.mistral.ai/v1/chat/completions",
            &body,
            &[
                ("Authorization", &format!("Bearer {}", api_key)),
                ("Content-Type", "application/json"),
            ],
        )?;

        let text = extract_chat_completion_text(&response, "Mistral")?;
        all_summaries.push(text);
    }

    // If multiple chunks, do a final synthesis
    let final_text = if all_summaries.len() > 1 {
        let combined = all_summaries.join("\n\n---\n\n");
        let synth_body = serde_json::json!({
            "model": model,
            "messages": [
                { "role": "system", "content": "Combine these partial meeting summaries into a single cohesive summary. Use the same KEY POINTS / DECISIONS / ACTION ITEMS format." },
                { "role": "user", "content": format!("Combine these summaries:\n\n{}", combined) }
            ],
            "max_tokens": 1024,
        });

        let response = http_post(
            "https://api.mistral.ai/v1/chat/completions",
            &synth_body,
            &[
                ("Authorization", &format!("Bearer {}", api_key)),
                ("Content-Type", "application/json"),
            ],
        )?;
        extract_chat_completion_text(&response, "Mistral")?
    } else {
        all_summaries.into_iter().next().unwrap_or_default()
    };

    Ok(parse_summary_response(&final_text))
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

        let text = response["response"]
            .as_str()
            .ok_or_else(|| format!("unexpected Ollama API response: {}", response))?;
        all_text.push_str(text);
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

// ── Screen context image encoding ────────────────────────────
// Reads PNG files, base64-encodes them, and formats for each LLM API.
// Limits to MAX_SCREEN_IMAGES to avoid blowing API token limits.

const MAX_SCREEN_IMAGES: usize = 8;

fn read_and_encode_images(screen_files: &[std::path::PathBuf]) -> Vec<(String, String)> {
    use base64::{engine::general_purpose::STANDARD, Engine};

    screen_files
        .iter()
        .take(MAX_SCREEN_IMAGES) // Limit to avoid API token limits
        .filter_map(|path| {
            std::fs::read(path).ok().map(|bytes| {
                let b64 = STANDARD.encode(&bytes);
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("screenshot.png")
                    .to_string();
                (name, b64)
            })
        })
        .collect()
}

/// Encode screenshots as Claude API image content blocks.
fn encode_screens_for_claude(screen_files: &[std::path::PathBuf]) -> Vec<serde_json::Value> {
    read_and_encode_images(screen_files)
        .into_iter()
        .map(|(_name, b64)| {
            serde_json::json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": "image/png",
                    "data": b64
                }
            })
        })
        .collect()
}

/// Encode screenshots as Mistral API image_url content blocks.
/// Mistral uses a flat `image_url` string (no nested object, no `detail` field).
fn encode_screens_for_mistral(screen_files: &[std::path::PathBuf]) -> Vec<serde_json::Value> {
    read_and_encode_images(screen_files)
        .into_iter()
        .map(|(_name, b64)| {
            serde_json::json!({
                "type": "image_url",
                "image_url": format!("data:image/png;base64,{}", b64)
            })
        })
        .collect()
}

/// Encode screenshots as OpenAI API image_url content blocks.
fn encode_screens_for_openai(screen_files: &[std::path::PathBuf]) -> Vec<serde_json::Value> {
    read_and_encode_images(screen_files)
        .into_iter()
        .map(|(_name, b64)| {
            serde_json::json!({
                "type": "image_url",
                "image_url": {
                    "url": format!("data:image/png;base64,{}", b64),
                    "detail": "low"  // Use low detail to reduce token cost
                }
            })
        })
        .collect()
}

// ── Speaker mapping (Level 1) ────────────────────────────────

const SPEAKER_MAPPING_PROMPT: &str = r#"Given this meeting transcript with anonymous speaker labels (SPEAKER_1, SPEAKER_2, etc.) and a list of known attendees, determine which speaker is which person based on conversational context clues.

Look for: direct address, role mentions, self-references, topic ownership.

ATTENDEES:
{attendees}

TRANSCRIPT (first 3000 chars):
{transcript}

For each speaker, respond in this exact format (one per line):
SPEAKER_1 = Name
SPEAKER_2 = Name

If you cannot determine a speaker's identity, respond:
SPEAKER_X = UNKNOWN

Only output the mappings, nothing else."#;

/// Map anonymous speaker labels to real names using an LLM.
/// Returns Medium-confidence attributions.
pub fn map_speakers(
    transcript: &str,
    attendees: &[String],
    config: &Config,
) -> Vec<crate::diarize::SpeakerAttribution> {
    if attendees.is_empty() || !transcript.contains("SPEAKER_") {
        return Vec::new();
    }

    let speakers = extract_speaker_labels(transcript);
    if speakers.is_empty() {
        return Vec::new();
    }

    tracing::info!(
        speakers = speakers.len(),
        attendees = attendees.len(),
        "Level 1: LLM speaker mapping"
    );

    let max_chars = 3000;
    let truncated = if transcript.len() > max_chars {
        let mut end = max_chars;
        while end > 0 && !transcript.is_char_boundary(end) {
            end -= 1;
        }
        &transcript[..end]
    } else {
        transcript
    };

    let prompt = SPEAKER_MAPPING_PROMPT
        .replace("{attendees}", &attendees.join(", "))
        .replace("{transcript}", truncated);

    let response = if config.summarization.engine != "none" {
        run_speaker_mapping_prompt(&prompt, config)
    } else {
        run_speaker_mapping_via_agent(&prompt, config)
    };

    match response {
        Ok(text) => {
            let mappings = parse_speaker_mapping(&text, &speakers, attendees);
            if !mappings.is_empty() {
                tracing::info!(mapped = mappings.len(), "Level 1: speaker mapping complete");
            }
            mappings
        }
        Err(e) => {
            tracing::warn!(error = %e, "Level 1: speaker mapping failed");
            Vec::new()
        }
    }
}

/// Extract unique SPEAKER_X labels from a transcript. Public for pipeline use.
pub fn extract_speaker_labels_pub(transcript: &str) -> Vec<String> {
    extract_speaker_labels(transcript)
}

fn extract_speaker_labels(transcript: &str) -> Vec<String> {
    let mut labels = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in transcript.lines() {
        if let Some(rest) = line.strip_prefix('[') {
            if let Some(bracket_end) = rest.find(']') {
                let inside = &rest[..bracket_end];
                if let Some(space_pos) = inside.find(' ') {
                    let label = &inside[..space_pos];
                    if label.starts_with("SPEAKER_") && seen.insert(label.to_string()) {
                        labels.push(label.to_string());
                    }
                }
            }
        }
    }
    labels
}

fn run_speaker_mapping_prompt(
    prompt: &str,
    config: &Config,
) -> Result<String, Box<dyn std::error::Error>> {
    match config.summarization.engine.as_str() {
        "agent" => run_speaker_mapping_via_agent(prompt, config),
        "claude" => {
            let api_key =
                std::env::var("ANTHROPIC_API_KEY").map_err(|_| "ANTHROPIC_API_KEY not set")?;
            let body = serde_json::json!({"model":"claude-sonnet-4-20250514","max_tokens":256,"messages":[{"role":"user","content":prompt}]});
            let resp: serde_json::Value = ureq::post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .send_json(&body)?
                .body_mut()
                .read_json()?;
            resp["content"][0]["text"]
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| "No text in response".into())
        }
        "openai" => {
            let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| "OPENAI_API_KEY not set")?;
            let body = serde_json::json!({"model":"gpt-4o-mini","max_tokens":256,"messages":[{"role":"user","content":prompt}]});
            let resp: serde_json::Value = ureq::post("https://api.openai.com/v1/chat/completions")
                .header("Authorization", &format!("Bearer {}", api_key))
                .header("content-type", "application/json")
                .send_json(&body)?
                .body_mut()
                .read_json()?;
            resp["choices"][0]["message"]["content"]
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| "No text in response".into())
        }
        "mistral" => {
            let api_key =
                std::env::var("MISTRAL_API_KEY").map_err(|_| "MISTRAL_API_KEY not set")?;
            let body = serde_json::json!({"model": &config.summarization.mistral_model, "max_tokens": 256, "messages":[{"role":"user","content":prompt}]});
            let resp: serde_json::Value = ureq::post("https://api.mistral.ai/v1/chat/completions")
                .header("Authorization", &format!("Bearer {}", api_key))
                .header("content-type", "application/json")
                .send_json(&body)?
                .body_mut()
                .read_json()?;
            resp["choices"][0]["message"]["content"]
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| "No text in response".into())
        }
        "ollama" => {
            let url = format!("{}/api/generate", config.summarization.ollama_url);
            let body = serde_json::json!({"model": config.summarization.ollama_model, "prompt": prompt, "stream": false});
            let resp: serde_json::Value = ureq::post(&url)
                .header("content-type", "application/json")
                .send_json(&body)?
                .body_mut()
                .read_json()?;
            resp["response"]
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| "No text in response".into())
        }
        other => Err(format!("Unknown engine: {}", other).into()),
    }
}

fn run_speaker_mapping_via_agent(
    prompt: &str,
    config: &Config,
) -> Result<String, Box<dyn std::error::Error>> {
    use std::io::Write;
    let agent_cmd = if config.summarization.agent_command.is_empty() {
        "claude".to_string()
    } else {
        config.summarization.agent_command.clone()
    };
    let agent_cmd = resolve_agent_path(&agent_cmd);
    let (cmd, args): (&str, Vec<&str>) = if agent_cmd == "claude" || agent_cmd.ends_with("/claude")
    {
        (&agent_cmd, vec!["-p", "-", "--no-input"])
    } else if agent_cmd == "codex" || agent_cmd.ends_with("/codex") {
        (&agent_cmd, vec!["exec", "-", "-s", "read-only"])
    } else {
        (&agent_cmd, vec![])
    };
    let mut child = std::process::Command::new(cmd)
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Agent '{}' not found: {}", agent_cmd, e))?;
    if let Some(mut stdin) = child.stdin.take() {
        let bytes = prompt.as_bytes().to_vec();
        std::thread::spawn(move || {
            stdin.write_all(&bytes).ok();
        });
    }
    let timeout = std::time::Duration::from_secs(120);
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child.wait_with_output()?;
                if !status.success() {
                    return Err(format!(
                        "Agent failed: {}",
                        String::from_utf8_lossy(&output.stderr)
                    )
                    .into());
                }
                return Ok(String::from_utf8_lossy(&output.stdout).to_string());
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    child.kill().ok();
                    return Err("Agent timed out".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(e) => return Err(format!("Error: {}", e).into()),
        }
    }
}

fn parse_speaker_mapping(
    response: &str,
    valid_speakers: &[String],
    valid_attendees: &[String],
) -> Vec<crate::diarize::SpeakerAttribution> {
    let valid_set: std::collections::HashSet<&str> =
        valid_speakers.iter().map(|s| s.as_str()).collect();
    let attendee_lower: std::collections::HashSet<String> =
        valid_attendees.iter().map(|a| a.to_lowercase()).collect();
    let mut results = Vec::new();
    for line in response.lines() {
        let trimmed = line.trim();
        if let Some(eq_pos) = trimmed.find('=') {
            let label = trimmed[..eq_pos].trim();
            let name = trimmed[eq_pos + 1..].trim();
            if valid_set.contains(label)
                && !name.is_empty()
                && !name.eq_ignore_ascii_case("UNKNOWN")
            {
                let name_lower = name.to_lowercase();
                let matches_attendee = attendee_lower.iter().any(|a| {
                    a.contains(&name_lower)
                        || name_lower.contains(a.as_str())
                        || a.split_whitespace()
                            .any(|part| part.len() > 2 && name_lower.contains(part))
                });
                if matches_attendee {
                    results.push(crate::diarize::SpeakerAttribution {
                        speaker_label: label.to_string(),
                        name: name.to_string(),
                        confidence: crate::diarize::Confidence::Medium,
                        source: crate::diarize::AttributionSource::Llm,
                    });
                }
            }
        }
    }
    results
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
- @case: Review competitor grid

OPEN QUESTIONS:
- Do we grandfather current customers?

COMMITMENTS:
- @sarah: Share revised pricing model by Tuesday";

        let summary = parse_summary_response(response);
        assert_eq!(summary.key_points.len(), 2);
        assert_eq!(summary.decisions.len(), 1);
        assert_eq!(summary.action_items.len(), 2);
        assert_eq!(summary.open_questions.len(), 1);
        assert_eq!(summary.commitments.len(), 1);
        assert!(summary.action_items[0].contains("@user"));
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
    fn parse_summary_response_extracts_participants() {
        let response = "\
KEY POINTS:
- Discussed the patent

PARTICIPANTS:
- Dan (patent attorney)
- Catherine
- Mat (demo/dev)";

        let summary = parse_summary_response(response);
        assert_eq!(summary.participants.len(), 3);
        assert_eq!(summary.participants[0], "Dan");
        assert_eq!(summary.participants[1], "Catherine");
        assert_eq!(summary.participants[2], "Mat");
    }

    #[test]
    fn format_summary_produces_markdown() {
        let summary = Summary {
            text: String::new(),
            key_points: vec!["Point one".into(), "Point two".into()],
            decisions: vec!["Decision A".into()],
            action_items: vec!["@user: Do the thing".into()],
            open_questions: vec!["Should we grandfather current customers?".into()],
            commitments: vec!["@case: Share the rollout plan by Friday".into()],
            participants: vec!["User".into(), "Case".into()],
        };
        let md = format_summary(&summary);
        assert!(md.contains("- Point one"));
        assert!(md.contains("## Decisions"));
        assert!(md.contains("- [x] Decision A"));
        assert!(md.contains("## Action Items"));
        assert!(md.contains("- [ ] @user: Do the thing"));
        assert!(md.contains("## Open Questions"));
        assert!(md.contains("## Commitments"));
    }

    #[test]
    fn summarize_returns_none_when_disabled() {
        let config = Config::default(); // engine = "none"
        let result = summarize("some transcript", &config);
        assert!(result.is_none());
    }

    #[test]
    fn extract_speaker_labels_finds_unique() {
        let t = "[SPEAKER_1 0:00] Hi\n[SPEAKER_2 0:05] Hey\n[SPEAKER_1 0:10] Ok\n";
        assert_eq!(extract_speaker_labels(t), vec!["SPEAKER_1", "SPEAKER_2"]);
    }

    #[test]
    fn extract_speaker_labels_ignores_named() {
        assert_eq!(
            extract_speaker_labels("[Mat 0:00] Hi\n[SPEAKER_1 0:05] Hey\n"),
            vec!["SPEAKER_1"]
        );
    }

    #[test]
    fn parse_speaker_mapping_valid() {
        let r = "SPEAKER_1 = Alex Chen\nSPEAKER_2 = Sarah Kim\n";
        let s = vec!["SPEAKER_1".into(), "SPEAKER_2".into()];
        let a = vec!["Alex Chen".into(), "Sarah Kim".into()];
        let result = parse_speaker_mapping(r, &s, &a);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "Alex Chen");
        assert_eq!(result[0].confidence, crate::diarize::Confidence::Medium);
    }

    #[test]
    fn parse_speaker_mapping_skips_unknown() {
        let r = "SPEAKER_1 = Alex\nSPEAKER_2 = UNKNOWN\n";
        let result = parse_speaker_mapping(
            r,
            &["SPEAKER_1".into(), "SPEAKER_2".into()],
            &["Alex Chen".into()],
        );
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn parse_speaker_mapping_rejects_hallucinated() {
        let result =
            parse_speaker_mapping("SPEAKER_1 = Bob\n", &["SPEAKER_1".into()], &["Alex".into()]);
        assert!(result.is_empty());
    }

    #[test]
    fn map_speakers_empty_when_no_speakers() {
        let config = Config::default();
        assert!(map_speakers("[0:00] no labels", &["Alex".into()], &config).is_empty());
    }

    #[test]
    fn map_speakers_empty_when_no_attendees() {
        let config = Config::default();
        assert!(map_speakers("[SPEAKER_1 0:00] hi", &[], &config).is_empty());
    }
}
