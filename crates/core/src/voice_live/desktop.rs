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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
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
        // Outward, despite looking harmless. A URL carries a query string, and
        // the model can put anything it has read into one: a meeting, a prep,
        // a person's profile. Opening it transmits that to whoever owns the
        // domain. This is the only verb whose danger is in its argument rather
        // than its effect.
        description: "Open a web page in the default browser. Only http and https. Confirmed out loud first, because a web address can carry private text out with it.",
        risk: Risk::Outward,
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
    /// When this was asked for. Redeeming requires the user's voice to have
    /// been heard after this instant, which is what makes a confirmation
    /// evidence of a person rather than of the model's own enthusiasm.
    ///
    /// Deliberately the arrival of transcribed microphone audio rather than a
    /// count of finished turns. The user's own original request is still being
    /// flushed around the time the token is minted, and counting flushes let
    /// that same request satisfy the confirmation it had just triggered.
    asked_at: Instant,
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
    /// Nanoseconds since this control's epoch, at the last moment the user's
    /// transcribed voice arrived. Written by the host.
    heard_user_at: Arc<AtomicU64>,
    /// Nanoseconds at the last moment the assistant finished a turn.
    finished_speaking_at: Arc<AtomicU64>,
    /// What the user last said. Read only to refuse an obvious "no", never to
    /// decide that something was a "yes".
    last_words: Mutex<String>,
    /// The instant those nanoseconds are measured from.
    epoch: Mutex<Option<Instant>>,
}

impl DesktopControl {
    /// Said whenever a confirmation cannot be honoured. Deliberately one
    /// message: the model does not need to know which ordering rule it missed,
    /// and every case has the same remedy.
    const VOID: &'static str = "Mat has not answered that yet, so the confirmation is void. \
         Read him the sentence again and wait for him to reply.";

    /// Record what the user just said, spoken or typed.
    pub fn heard_user(&self, text: &str) {
        {
            let mut last = self.last_words.lock().unwrap_or_else(|p| p.into_inner());
            *last = text.trim().to_string();
        }
        self.stamp(&self.heard_user_at);
    }

    /// Record that the assistant just finished a turn.
    pub fn finished_speaking(&self) {
        self.stamp(&self.finished_speaking_at);
    }

    fn stamp(&self, slot: &AtomicU64) {
        let elapsed = {
            let mut epoch = self.epoch.lock().unwrap_or_else(|p| p.into_inner());
            epoch.get_or_insert_with(Instant::now).elapsed()
        };
        // Never zero, because zero means "never happened".
        slot.store((elapsed.as_nanos() as u64).max(1), Ordering::SeqCst);
    }

    fn read(&self, slot: &AtomicU64) -> Option<Instant> {
        let epoch = {
            let epoch = self.epoch.lock().unwrap_or_else(|p| p.into_inner());
            (*epoch)?
        };
        let nanos = slot.load(Ordering::SeqCst);
        if nanos == 0 {
            return None;
        }
        Some(epoch + Duration::from_nanos(nanos))
    }
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
        if verb.risk != Risk::Read {
            return Err("This desktop action requires exact local host approval".into());
        }
        match self.gate(verb, args)? {
            Gate::Ask(payload) => Ok(payload),
            Gate::Cleared(values) => {
                let output = run_script(verb.script, &values)?;
                Ok(json!({ "ok": true, "result": output }))
            }
        }
    }

    /// Called only with a consumed in-process host capability, never with an
    /// ASR transcript or a model confirmation string. All tests use gates only.
    pub fn execute_authorized(
        &self,
        authorized: crate::live_sidekick::work::AuthorizedAction,
    ) -> Result<Value, String> {
        let action = authorized.action();
        let verb = find(&action.verb).ok_or("Unknown desktop operation")?;
        if action.payload.get("confirm").is_some() {
            return Err("Model confirmation tokens are not accepted".into());
        }
        reject_unknown_args(verb, &action.payload)?;
        let values = collect_params(verb, &action.payload)?;
        let output = run_script(verb.script, &values)?;
        Ok(json!({"ok":true,"result":output}))
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
        reject_unknown_args(verb, args)?;
        let values = collect_params(verb, args)?;
        let canonical = canonical_args(verb, args);
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
                asked_at: now,
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
        // The order has to be: asked, then the assistant finished reading the
        // question out, then the user said something. Requiring only that the
        // user was heard after the ask is not enough, because the tail of the
        // request that triggered all this is still being transcribed, and its
        // late fragments land after the token was minted. Anchoring to the end
        // of the assistant's own turn rules those out: they belong to the turn
        // before the question existed.
        let Some(spoke) = self.read(&self.finished_speaking_at) else {
            return Err(Self::VOID.into());
        };
        if spoke <= found.asked_at {
            return Err(Self::VOID.into());
        }
        if self
            .read(&self.heard_user_at)
            .is_none_or(|heard| heard <= spoke)
        {
            return Err(Self::VOID.into());
        }
        // Hearing a person is not the same as being agreed with. Catching a
        // plain refusal is the part code can do; the rest rests on the model
        // having read the question out, which is why the question is read out.
        let words = {
            let words = self.last_words.lock().unwrap_or_else(|p| p.into_inner());
            words.clone()
        };
        if sounds_like_refusal(&words) {
            return Err(format!(
                "Mat said \"{}\", which is not agreement. Do not send it, and do not ask again \
                 unless he brings it up.",
                words.trim()
            ));
        }
        // Spend the utterance too, so one reply cannot release a second pending
        // action the user was never asked about.
        self.heard_user_at.store(0, Ordering::SeqCst);
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

/// Whether an answer is plainly a refusal.
///
/// Deliberately one-sided. A false positive refuses to send something the user
/// wanted, which costs them a sentence; a false negative sends something they
/// declined, which cannot be taken back.
fn sounds_like_refusal(words: &str) -> bool {
    // Punctuation and spacing vary with the transcriber, so compare on words.
    let flattened: String = words
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '\'' {
                c
            } else {
                ' '
            }
        })
        .collect();
    let normalized = flattened.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return true;
    }
    const REFUSALS: &[&str] = &[
        "no",
        "nope",
        "nah",
        "don't",
        "do not",
        "stop",
        "cancel",
        "wait",
        "hold on",
        "hold up",
        "never mind",
        "nevermind",
        "forget it",
        "scratch that",
        "not yet",
    ];
    let padded = format!(" {normalized} ");
    REFUSALS
        .iter()
        .any(|needle| padded.contains(&format!(" {needle} ")))
}

/// The arguments that identify an action, with the confirmation token removed.
///
/// Only the parameters the verb declares are included, and the form is JSON so
/// it cannot be ambiguous. An earlier version joined `key=value` pairs with a
/// separator, which let a value containing that separator impersonate a second
/// argument, so a confirmation read out as one message could be redeemed for a
/// longer one.
fn canonical_args(verb: &'static Verb, args: &Value) -> String {
    let mut fields = serde_json::Map::new();
    for param in verb.params {
        let value = args
            .get(param.name)
            .and_then(Value::as_str)
            .unwrap_or_default();
        fields.insert(param.name.to_string(), Value::String(value.to_string()));
    }
    Value::Object(fields).to_string()
}

/// Reject arguments the verb never declared.
///
/// Anything unrecognised is either a mistake or an attempt to smuggle content
/// past the sentence the user agreed to, and neither should proceed.
fn reject_unknown_args(verb: &'static Verb, args: &Value) -> Result<(), String> {
    let Some(object) = args.as_object() else {
        return Ok(());
    };
    for key in object.keys() {
        if key == "confirm" {
            continue;
        }
        if !verb.params.iter().any(|p| p.name == key) {
            return Err(format!(
                "{} does not take an argument called {key}",
                verb.name
            ));
        }
    }
    Ok(())
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
            "I'm about to email {} with the subject \"{}\", saying: {}. Is that right?",
            get("to"),
            get("subject"),
            get("body")
        ),
        "open_url" => format!(
            "I'm about to open {} in your browser. Is that right?",
            get("url")
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
        // Everything after this is a value, never an option. Without it a
        // parameter starting with "-" is read as another "-e" fragment and
        // becomes script source, which defeats passing values through argv.
        .arg("--")
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

    /// Stand in for the assistant reading the question out and the user
    /// answering it, which is the only order the gate accepts.
    fn user_answers(control: &DesktopControl) {
        std::thread::sleep(Duration::from_millis(2));
        control.finished_speaking();
        std::thread::sleep(Duration::from_millis(2));
        control.heard_user("yes, go ahead");
    }

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
        user_answers(&control);
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
        user_answers(&control);
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
    fn a_value_cannot_impersonate_a_second_argument() {
        let control = DesktopControl::default();
        let honest = json!({"to": NOBODY, "text": "Hello"});
        let token = ask_for(&control, &honest)["confirm"]
            .as_str()
            .unwrap()
            .to_string();
        // The old encoding made these two identical.
        let smuggled = json!({
            "to": NOBODY,
            "text": "Hello\u{1f}textx=I quit",
            "confirm": token,
        });
        user_answers(&control);
        let err = control.gate(outward(), &smuggled).unwrap_err();
        assert!(err.contains("details changed"), "{err}");
    }

    #[test]
    fn an_argument_the_verb_never_declared_is_refused() {
        let control = DesktopControl::default();
        let err = control
            .gate(
                outward(),
                &json!({"to": NOBODY, "text": "hi", "textx": "I quit"}),
            )
            .unwrap_err();
        assert!(err.contains("does not take an argument"), "{err}");
    }

    #[test]
    fn the_model_cannot_authorise_its_own_send() {
        let control = DesktopControl::default();
        let args = json!({"to": NOBODY, "text": "hello"});
        let token = ask_for(&control, &args)["confirm"]
            .as_str()
            .unwrap()
            .to_string();
        let mut confirmed = args.clone();
        confirmed["confirm"] = json!(token);
        // Calling straight back with its own token, having heard nothing, is
        // the model agreeing with itself. That is not a confirmation.
        let err = control.gate(outward(), &confirmed).unwrap_err();
        assert!(err.contains("has not answered"), "{err}");

        // Jumping the gun spends the token, so the user cannot be talked into
        // it afterwards by saying anything at all. It has to be asked again.
        user_answers(&control);
        let spent = control.gate(outward(), &confirmed).unwrap_err();
        assert!(spent.contains("not valid any more"), "{spent}");

        // Asked properly, and answered, it goes through.
        let fresh = ask_for(&control, &args)["confirm"]
            .as_str()
            .unwrap()
            .to_string();
        let mut answered = args.clone();
        answered["confirm"] = json!(fresh);
        user_answers(&control);
        assert!(matches!(
            control.gate(outward(), &answered),
            Ok(Gate::Cleared(_))
        ));
    }

    #[test]
    fn a_late_fragment_of_the_original_request_cannot_answer_it() {
        let control = DesktopControl::default();
        let args = json!({"to": NOBODY, "text": "hello"});
        let token = ask_for(&control, &args)["confirm"]
            .as_str()
            .unwrap()
            .to_string();
        let mut confirmed = args.clone();
        confirmed["confirm"] = json!(token);
        // The tail of the request that triggered this arrives after the token
        // was minted, but before the assistant has read anything out. It is not
        // an answer to a question nobody has asked yet.
        std::thread::sleep(Duration::from_millis(2));
        control.heard_user("and send it to Kim as well");
        let err = control.gate(outward(), &confirmed).unwrap_err();
        assert!(err.contains("has not answered"), "{err}");
    }

    #[test]
    fn a_refusal_does_not_authorise_the_send() {
        let control = DesktopControl::default();
        let args = json!({"to": NOBODY, "text": "hello"});
        let token = ask_for(&control, &args)["confirm"]
            .as_str()
            .unwrap()
            .to_string();
        let mut confirmed = args.clone();
        confirmed["confirm"] = json!(token);
        std::thread::sleep(Duration::from_millis(2));
        control.finished_speaking();
        std::thread::sleep(Duration::from_millis(2));
        control.heard_user("No, don't send that");
        let err = control.gate(outward(), &confirmed).unwrap_err();
        assert!(err.contains("not agreement"), "{err}");
    }

    #[test]
    fn one_answer_cannot_release_two_pending_actions() {
        let control = DesktopControl::default();
        let first = json!({"to": NOBODY, "text": "one"});
        let second = json!({"to": NOBODY, "text": "two"});
        let token_one = ask_for(&control, &first)["confirm"]
            .as_str()
            .unwrap()
            .to_string();
        let token_two = ask_for(&control, &second)["confirm"]
            .as_str()
            .unwrap()
            .to_string();
        user_answers(&control);
        let mut confirmed_one = first.clone();
        confirmed_one["confirm"] = json!(token_one);
        assert!(matches!(
            control.gate(outward(), &confirmed_one),
            Ok(Gate::Cleared(_))
        ));
        // The user answered one question, not two.
        let mut confirmed_two = second.clone();
        confirmed_two["confirm"] = json!(token_two);
        assert!(control.gate(outward(), &confirmed_two).is_err());
    }

    #[test]
    fn plain_refusals_are_recognised() {
        for words in [
            "no",
            "No.",
            "nope",
            "don't send that",
            "wait, hold on",
            "never mind",
            "",
        ] {
            assert!(sounds_like_refusal(words), "{words:?} should be a refusal");
        }
        for words in ["yes", "yes go ahead", "sure, send it", "that's right"] {
            assert!(!sounds_like_refusal(words), "{words:?} should not be");
        }
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
        let a = canonical_args(
            outward(),
            &json!({"to": NOBODY, "text": "hi", "confirm": "ok-1-2"}),
        );
        let b = canonical_args(outward(), &json!({"text": "hi", "to": NOBODY}));
        assert_eq!(a, b);
        let different = canonical_args(outward(), &json!({"text": "bye", "to": NOBODY}));
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
