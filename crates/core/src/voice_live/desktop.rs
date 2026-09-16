//! Desktop actions, as a fixed catalogue of verbs.
//!
//! The model picks a verb and fills typed parameters. It never writes
//! AppleScript. A tool taking script text would be a remote shell, because
//! `do shell script` inside AppleScript is arbitrary execution, and the caller
//! here is a speech model that has been observed acting on a misheard fragment
//! and on noise a transcriber turned into words.
//!
//! Parameters reach osascript through `argv`, never by string interpolation, so
//! there is no script injection to reason about: a parameter is a value to the
//! interpreter, never source.
//!
//! Anything that leaves the machine or is hard to undo is two-phase. The first
//! call performs nothing. It returns a sentence to say out loud and a
//! single-use token bound to those exact parameters, and only a second call
//! carrying that token acts. The gate is here in code rather than in the
//! prompt, because a prompt asking the model to confirm first is precisely the
//! control that already failed: told not to write without asking, it opened a
//! GitHub issue from one spoken sentence.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

/// How long a spoken confirmation stays good for.
const CONFIRM_WINDOW: Duration = Duration::from_secs(120);
/// Never let pending confirmations accumulate.
const MAX_PENDING: usize = 8;
/// Bound on any single action, so a stuck script cannot wedge the tool worker.
const ACTION_TIMEOUT: Duration = Duration::from_secs(20);

/// How much a verb can cost you if it fires when you did not mean it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    /// Reads state and changes nothing.
    Read,
    /// Changes something local that you can trivially undo.
    Local,
    /// Leaves the machine, or is not easily undone. Two-phase.
    Outward,
}

/// One typed parameter of a verb.
pub struct Param {
    pub name: &'static str,
    pub description: &'static str,
    pub required: bool,
}

/// One thing the assistant can do.
pub struct Verb {
    pub name: &'static str,
    pub description: &'static str,
    pub risk: Risk,
    pub params: &'static [Param],
    /// AppleScript body. Parameters arrive as `argv`, in the order declared.
    script: &'static str,
}

const fn p(name: &'static str, description: &'static str, required: bool) -> Param {
    Param {
        name,
        description,
        required,
    }
}

/// Everything the assistant may do. Adding to this list is the only way to
/// widen what it can reach.
pub const VERBS: &[Verb] = &[
    Verb {
        name: "now_playing",
        description: "What music is playing right now, if anything.",
        risk: Risk::Read,
        params: &[],
        script: r#"on run argv
    set out to "nothing is playing"
    try
        if application "Music" is running then
            tell application "Music"
                if player state is playing then
                    set out to (name of current track) & " by " & (artist of current track) & " in Music"
                end if
            end tell
        end if
    end try
    try
        if application "Spotify" is running then
            tell application "Spotify"
                if player state is playing then
                    set out to (name of current track) & " by " & (artist of current track) & " in Spotify"
                end if
            end tell
        end if
    end try
    return out
end run"#,
    },
    Verb {
        name: "control_music",
        description: "Play, pause or skip whatever is playing.",
        risk: Risk::Local,
        params: &[p(
            "action",
            "One of: play, pause, next, previous",
            true,
        )],
        script: r#"on run argv
    set act to item 1 of argv
    set app_name to "Music"
    try
        if application "Spotify" is running then set app_name to "Spotify"
    end try
    tell application app_name
        if act is "play" then
            play
        else if act is "pause" then
            pause
        else if act is "next" then
            next track
        else if act is "previous" then
            previous track
        else
            error "unknown action"
        end if
    end tell
    return act & " in " & app_name
end run"#,
    },
    Verb {
        name: "open_app",
        description: "Bring an application to the front, launching it if needed.",
        risk: Risk::Local,
        params: &[p("name", "The application name, for example Safari", true)],
        script: r#"on run argv
    tell application (item 1 of argv) to activate
    return "opened " & (item 1 of argv)
end run"#,
    },
    Verb {
        name: "open_url",
        description: "Open a web page in the default browser. Only http and https.",
        risk: Risk::Local,
        params: &[p("url", "An http or https address", true)],
        script: r#"on run argv
    open location (item 1 of argv)
    return "opened " & (item 1 of argv)
end run"#,
    },
    Verb {
        name: "reveal_path",
        description: "Show a file or folder in the Finder. Use it for a meeting file or a repository.",
        risk: Risk::Local,
        params: &[p("path", "An absolute path on this machine", true)],
        script: r#"on run argv
    tell application "Finder"
        reveal POSIX file (item 1 of argv) as alias
        activate
    end tell
    return "revealed " & (item 1 of argv)
end run"#,
    },
    Verb {
        name: "add_reminder",
        description: "Add a reminder. Local to this machine and trivially deleted, so it does not need confirming.",
        risk: Risk::Local,
        params: &[
            p("text", "What to be reminded of", true),
            p("minutes_from_now", "When to fire, in minutes. Omit for no alarm.", false),
        ],
        script: r#"on run argv
    set body to item 1 of argv
    set mins to item 2 of argv
    tell application "Reminders"
        if mins is "" then
            make new reminder with properties {name:body}
        else
            set due to (current date) + ((mins as integer) * minutes)
            make new reminder with properties {name:body, remind me date:due}
        end if
    end tell
    return "reminder added"
end run"#,
    },
    Verb {
        name: "send_message",
        description: "Send an iMessage. This leaves the machine, so it is confirmed out loud first.",
        risk: Risk::Outward,
        params: &[
            p("to", "Phone number, email address, or exact contact name", true),
            p("text", "The message to send", true),
        ],
        script: r#"on run argv
    tell application "Messages"
        set svc to 1st account whose service type = iMessage
        set who to participant (item 1 of argv) of svc
        send (item 2 of argv) to who
    end tell
    return "sent"
end run"#,
    },
    Verb {
        name: "send_email",
        description: "Send an email. This leaves the machine, so it is confirmed out loud first.",
        risk: Risk::Outward,
        params: &[
            p("to", "Recipient email address", true),
            p("subject", "Subject line", true),
            p("body", "Message body", true),
        ],
        script: r#"on run argv
    tell application "Mail"
        set msg to make new outgoing message with properties {subject:(item 2 of argv), content:(item 3 of argv), visible:false}
        tell msg to make new to recipient at end of to recipients with properties {address:(item 1 of argv)}
        send msg
    end tell
    return "sent"
end run"#,
    },
];

/// Look a verb up by the name the model used.
pub fn find(name: &str) -> Option<&'static Verb> {
    VERBS.iter().find(|v| v.name == name)
}

/// A confirmation the user has been asked for but has not yet given.
struct Pending {
    token: String,
    verb: &'static str,
    /// The exact arguments confirmed, so a different action cannot ride the
    /// token that was read out loud.
    canonical: String,
    created: Instant,
}

/// The outcome of the confirmation gate.
#[derive(Debug)]
pub(crate) enum Gate {
    /// Nothing happened. This payload must be read to the user.
    Ask(Value),
    /// Cleared to act, with the parameters in declaration order.
    Cleared(Vec<String>),
}

/// Holds what is awaiting spoken confirmation.
#[derive(Default)]
pub struct DesktopControl {
    pending: Mutex<Vec<Pending>>,
    counter: Mutex<u64>,
}

impl DesktopControl {
    /// Declarations for every verb this build allows.
    pub fn declarations(&self, outward_allowed: bool) -> Vec<Value> {
        VERBS
            .iter()
            .filter(|v| outward_allowed || v.risk != Risk::Outward)
            .map(|v| {
                let mut properties = Map::new();
                for param in v.params {
                    properties.insert(
                        param.name.to_string(),
                        json!({ "type": "string", "description": param.description }),
                    );
                }
                if v.risk == Risk::Outward {
                    properties.insert(
                        "confirm".to_string(),
                        json!({
                            "type": "string",
                            "description": "Leave this out on the first call. Call once without it, read the returned sentence to Mat, wait for him to agree, then call again with the token you were given and identical arguments.",
                        }),
                    );
                }
                let required: Vec<&str> = v
                    .params
                    .iter()
                    .filter(|p| p.required)
                    .map(|p| p.name)
                    .collect();
                let mut params = json!({ "type": "object", "properties": Value::Object(properties) });
                if !required.is_empty() {
                    params["required"] = json!(required);
                }
                json!({
                    "name": v.name,
                    "description": v.description,
                    "parameters": params,
                    "behavior": "NON_BLOCKING",
                })
            })
            .collect()
    }

    /// Run a verb, or ask for confirmation first when it needs one.
    pub fn execute(&self, verb: &'static Verb, args: &Value) -> Result<Value, String> {
        match self.gate(verb, args)? {
            Gate::Ask(payload) => Ok(payload),
            Gate::Cleared(values) => {
                let output = run_script(verb.script, &values)?;
                Ok(json!({ "ok": true, "result": output }))
            }
        }
    }

    /// Everything that decides whether an action may happen, and nothing that
    /// makes it happen.
    ///
    /// Split out so the gate can be tested without a test being able to perform
    /// a real action. That is not hypothetical: an earlier version of these
    /// tests redeemed a token and then genuinely attempted an iMessage on every
    /// run.
    pub(crate) fn gate(&self, verb: &'static Verb, args: &Value) -> Result<Gate, String> {
        // Validate first, so a confirmation is never read out for an action
        // that could not have run. Asking about an empty recipient and only
        // failing on the second call wastes the user's agreement.
        let values = collect_params(verb, args)?;
        let canonical = canonical_args(args);
        if verb.risk == Risk::Outward {
            match args.get("confirm").and_then(Value::as_str) {
                Some(token) => self.redeem(verb, token, &canonical)?,
                None => return Ok(Gate::Ask(self.ask(verb, &canonical, args))),
            }
        }
        Ok(Gate::Cleared(values))
    }

    /// Record what we are about to do and hand back a sentence to say.
    fn ask(&self, verb: &'static Verb, canonical: &str, args: &Value) -> Value {
        let token = self.mint();
        {
            let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
            let now = Instant::now();
            pending.retain(|p| now.duration_since(p.created) < CONFIRM_WINDOW);
            while pending.len() >= MAX_PENDING {
                pending.remove(0);
            }
            pending.push(Pending {
                token: token.clone(),
                verb: verb.name,
                canonical: canonical.to_string(),
                created: now,
            });
        }
        json!({
            "needs_confirmation": true,
            "say": describe(verb, args),
            "confirm": token,
            "note": "Nothing has happened yet. Read the sentence in `say` to Mat exactly as it is, wait for him to agree out loud, then call this tool again with the same arguments plus this confirm token. If he declines or changes anything, do not call again with this token.",
        })
    }

    /// Spend a token, or explain why it is no good.
    fn redeem(&self, verb: &'static Verb, token: &str, canonical: &str) -> Result<(), String> {
        let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();
        pending.retain(|p| now.duration_since(p.created) < CONFIRM_WINDOW);
        let Some(index) = pending.iter().position(|p| p.token == token) else {
            return Err(
                "that confirmation is not valid any more. Ask Mat again and use the new token."
                    .into(),
            );
        };
        // Single use, whether or not the rest checks out.
        let found = pending.remove(index);
        if found.verb != verb.name {
            return Err("that confirmation was for a different action".into());
        }
        if found.canonical != canonical {
            return Err(
                "the details changed since Mat confirmed. Read him the new ones and ask again."
                    .into(),
            );
        }
        Ok(())
    }

    fn mint(&self) -> String {
        let mut counter = self.counter.lock().unwrap_or_else(|p| p.into_inner());
        *counter += 1;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or_default();
        format!("ok-{}-{}", *counter, nanos)
    }
}

/// The arguments that identify an action, with any confirmation token removed
/// and keys ordered, so the same request always produces the same string.
fn canonical_args(args: &Value) -> String {
    let mut pairs: Vec<(String, String)> = args
        .as_object()
        .map(|o| {
            o.iter()
                .filter(|(k, _)| k.as_str() != "confirm")
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| v.to_string()),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    pairs.sort();
    pairs
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

/// The sentence the assistant reads out before doing something outward.
fn describe(verb: &'static Verb, args: &Value) -> String {
    let get = |name: &str| {
        args.get(name)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    match verb.name {
        "send_message" => format!(
            "I'm about to send \"{}\" to {}. Is that right?",
            get("text"),
            get("to")
        ),
        "send_email" => format!(
            "I'm about to email {} with the subject \"{}\". Is that right?",
            get("to"),
            get("subject")
        ),
        other => format!("I'm about to run {other}. Is that right?"),
    }
}

/// Pull the declared parameters out in order, validating as we go.
fn collect_params(verb: &'static Verb, args: &Value) -> Result<Vec<String>, String> {
    let mut values = Vec::with_capacity(verb.params.len());
    for param in verb.params {
        let raw = args
            .get(param.name)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if raw.is_empty() {
            if param.required {
                return Err(format!("{} is required", param.name));
            }
            values.push(String::new());
            continue;
        }
        values.push(validate(verb.name, param.name, raw)?);
    }
    Ok(values)
}

/// Per-parameter checks. Injection is already impossible because values travel
/// as `argv`, so these are about the action making sense, not about escaping.
fn validate(verb: &str, param: &str, raw: &str) -> Result<String, String> {
    if raw.chars().count() > 4_000 {
        return Err(format!("{param} is too long"));
    }
    match (verb, param) {
        ("open_url", "url") => {
            let lower = raw.to_ascii_lowercase();
            if !(lower.starts_with("http://") || lower.starts_with("https://")) {
                return Err("only http and https addresses can be opened".into());
            }
        }
        ("reveal_path", "path") => {
            if !raw.starts_with('/') {
                return Err("the path must be absolute".into());
            }
            if !std::path::Path::new(raw).exists() {
                return Err(format!("there is nothing at {raw}"));
            }
        }
        ("control_music", "action") if !["play", "pause", "next", "previous"].contains(&raw) => {
            return Err("action must be play, pause, next or previous".into());
        }
        ("add_reminder", "minutes_from_now") => {
            let minutes: i64 = raw
                .parse()
                .map_err(|_| "minutes_from_now must be a whole number".to_string())?;
            if !(0..=60 * 24 * 30).contains(&minutes) {
                return Err("minutes_from_now is out of range".into());
            }
        }
        _ => {}
    }
    Ok(raw.to_string())
}

/// Run an AppleScript with parameters as `argv`.
#[cfg(target_os = "macos")]
fn run_script(script: &str, values: &[String]) -> Result<String, String> {
    use std::io::Read;
    // No test may ever perform a real desktop action. An earlier version of
    // these tests redeemed a confirmation token and then genuinely tried to
    // send an iMessage on every `cargo test`. Tests exercise the gate; anything
    // reaching here under test is a bug, and failing loudly beats acting.
    if cfg!(test) {
        return Err("desktop actions never run under test".into());
    }
    let mut child = crate::engine_process::command("osascript")
        .args(["-e", script])
        .args(values)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run osascript: {e}"))?;

    let deadline = Instant::now() + ACTION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                let mut err = String::new();
                if let Some(mut s) = child.stdout.take() {
                    let _ = s.read_to_string(&mut out);
                }
                if let Some(mut s) = child.stderr.take() {
                    let _ = s.read_to_string(&mut err);
                }
                if !status.success() {
                    let err = err.trim();
                    // The permission prompt is the common first failure.
                    let hint = if err.contains("-1743")
                        || err.to_lowercase().contains("not allowed")
                    {
                        " Minutes needs permission to control that app, under Privacy and Security, Automation."
                    } else {
                        ""
                    };
                    return Err(format!("that did not work: {err}{hint}"));
                }
                return Ok(out.trim().to_string());
            }
            Ok(None) => {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("that action did not finish in time".into());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("could not wait for osascript: {e}")),
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn run_script(_script: &str, _values: &[String]) -> Result<String, String> {
    Err("desktop actions are macOS only for now".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outward() -> &'static Verb {
        find("send_message").expect("send_message exists")
    }

    /// A number reserved for fiction, so no test names anyone real.
    const NOBODY: &str = "555-0100";

    /// Ask the gate, expecting it to want confirmation, and return the payload.
    fn ask_for(control: &DesktopControl, args: &Value) -> Value {
        match control
            .gate(outward(), args)
            .expect("gate should not error")
        {
            Gate::Ask(payload) => payload,
            Gate::Cleared(_) => panic!("an outward verb cleared without confirmation"),
        }
    }

    #[test]
    fn no_test_can_perform_a_real_action() {
        assert!(run_script("on run argv\nreturn \"x\"\nend run", &[]).is_err());
    }

    #[test]
    fn an_outward_verb_never_acts_on_the_first_call() {
        let control = DesktopControl::default();
        let args = json!({"to": NOBODY, "text": "running five late"});
        let asked = ask_for(&control, &args);
        assert_eq!(asked["needs_confirmation"], true);
        // The sentence must contain what is actually going to happen.
        let say = asked["say"].as_str().unwrap();
        assert!(
            say.contains("running five late") && say.contains(NOBODY),
            "{say}"
        );
        assert!(asked["confirm"].as_str().is_some_and(|t| !t.is_empty()));
    }

    #[test]
    fn a_token_is_single_use() {
        let control = DesktopControl::default();
        let args = json!({"to": NOBODY, "text": "hello"});
        let token = ask_for(&control, &args)["confirm"]
            .as_str()
            .unwrap()
            .to_string();
        let mut confirmed = args.clone();
        confirmed["confirm"] = json!(token);
        // Spending it clears the gate exactly once. Nothing is performed: the
        // gate stops at the decision.
        assert!(matches!(
            control.gate(outward(), &confirmed),
            Ok(Gate::Cleared(_))
        ));
        let second = control.gate(outward(), &confirmed).unwrap_err();
        assert!(second.contains("not valid any more"), "{second}");
    }

    #[test]
    fn a_token_cannot_carry_a_different_message() {
        let control = DesktopControl::default();
        let asked = control
            .execute(
                outward(),
                &json!({"to": NOBODY, "text": "running five late"}),
            )
            .unwrap();
        let token = asked["confirm"].as_str().unwrap().to_string();
        // Confirmed one thing, attempted another.
        let swapped = json!({"to": NOBODY, "text": "you are fired", "confirm": token});
        let err = control.gate(outward(), &swapped).unwrap_err();
        assert!(err.contains("details changed"), "{err}");
    }

    #[test]
    fn an_unusable_action_is_refused_before_it_is_read_out() {
        let control = DesktopControl::default();
        // No recipient: this can never run, so it must not become a sentence
        // the user is asked to agree to.
        let err = control
            .gate(outward(), &json!({"text": "hello"}))
            .unwrap_err();
        assert!(err.contains("required"), "{err}");
    }

    #[test]
    fn an_invented_token_is_refused() {
        let control = DesktopControl::default();
        let err = control
            .gate(
                outward(),
                &json!({"to": NOBODY, "text": "hi", "confirm": "ok-1-12345"}),
            )
            .unwrap_err();
        assert!(err.contains("not valid any more"), "{err}");
    }

    #[test]
    fn outward_verbs_disappear_when_they_are_not_allowed() {
        let control = DesktopControl::default();
        let names = |allowed| -> Vec<String> {
            control
                .declarations(allowed)
                .into_iter()
                .map(|d| d["name"].as_str().unwrap_or_default().to_string())
                .collect()
        };
        assert!(!names(false).contains(&"send_message".to_string()));
        assert!(names(false).contains(&"now_playing".to_string()));
        assert!(names(true).contains(&"send_message".to_string()));
    }

    #[test]
    fn parameters_are_checked_before_anything_runs() {
        assert!(validate("open_url", "url", "file:///etc/passwd").is_err());
        assert!(validate("open_url", "url", "javascript:alert(1)").is_err());
        assert!(validate("open_url", "url", "https://example.com").is_ok());
        assert!(validate("reveal_path", "path", "relative/path").is_err());
        assert!(validate("reveal_path", "path", "/definitely/not/here").is_err());
        assert!(validate("control_music", "action", "destroy").is_err());
        assert!(validate("control_music", "action", "pause").is_ok());
        assert!(validate("add_reminder", "minutes_from_now", "abc").is_err());
        assert!(validate("add_reminder", "minutes_from_now", "-5").is_err());
        assert!(validate("add_reminder", "minutes_from_now", "30").is_ok());
    }

    #[test]
    fn canonical_arguments_ignore_the_token_and_key_order() {
        let a = canonical_args(&json!({"to": NOBODY, "text": "hi", "confirm": "ok-1-2"}));
        let b = canonical_args(&json!({"text": "hi", "to": NOBODY}));
        assert_eq!(a, b);
        let different = canonical_args(&json!({"text": "bye", "to": NOBODY}));
        assert_ne!(a, different);
    }

    #[test]
    fn every_verb_declares_what_it_needs() {
        for verb in VERBS {
            assert!(
                !verb.description.is_empty(),
                "{} has no description",
                verb.name
            );
            assert!(
                verb.script.contains("on run argv"),
                "{} takes no argv",
                verb.name
            );
            // Parameters arrive as argv, so the script must never build source
            // out of them.
            assert!(
                !verb.script.contains("do shell script"),
                "{} shells out",
                verb.name
            );
        }
    }
}
