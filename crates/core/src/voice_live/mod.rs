//! Voice Live: a push-to-talk spoken assistant over Minutes' memory (RFC 0007).
//!
//! The user holds a key, talks, and hears an answer. A realtime speech model
//! (phase 1: Gemini Live) does the listening and speaking; every fact comes from
//! a tool call into this crate. This module is an optional, cloud-backed,
//! failure-isolated consumer: it opens its own microphone stream, it refuses to
//! start while a recording is active, and nothing here can touch capture state.
//!
//! Layout: [`protocol`] speaks the wire format, [`audio_out`] plays model audio,
//! [`names`] biases and resolves people's names, [`tools`] dispatches into core,
//! and [`session`] ties them together on one thread per session.

pub mod audio_out;
pub mod decimate;
pub mod names;
pub mod protocol;
pub mod session;
pub mod tools;
#[cfg(target_os = "macos")]
pub mod voice_io;

use std::path::PathBuf;

use chrono::Local;
use thiserror::Error;

use crate::config::Config;

pub use names::NameIndex;
pub use session::{
    start, SessionOptions, TalkMode, VoiceLiveEvent, VoiceLiveSession, VoiceLiveState,
};
pub use tools::ToolContext;

/// Errors from starting or running a session.
#[derive(Debug, Error)]
pub enum VoiceLiveError {
    #[error("voice live is disabled; set [voice_live] enabled = true in config.toml")]
    Disabled,
    #[error("voice live sends microphone audio and tool results to {provider}; set [voice_live] allow_cloud = true to acknowledge")]
    CloudNotAllowed { provider: String },
    #[error("no API key: set the {0} environment variable")]
    MissingApiKey(String),
    #[error("a recording is active; voice live will not share the microphone with capture")]
    RecordingActive,
    #[error("connect: {0}")]
    Connect(String),
    #[error("audio: {0}")]
    Audio(String),
    #[error("session closed")]
    Closed,
}

/// Resolve the provider API key from the configured environment variable.
pub fn api_key(config: &Config) -> Result<String, VoiceLiveError> {
    let var = config.voice_live.api_key_env.trim();
    let var = if var.is_empty() {
        "GEMINI_API_KEY"
    } else {
        var
    };
    std::env::var(var)
        .ok()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .ok_or_else(|| VoiceLiveError::MissingApiKey(var.to_string()))
}

/// Everything that must hold before a host starts a session. Hosts call this
/// early so the user sees the reason instead of a failed connect.
pub fn preflight(config: &Config) -> Result<(), VoiceLiveError> {
    if !config.voice_live.enabled {
        return Err(VoiceLiveError::Disabled);
    }
    if !config.voice_live.allow_cloud {
        return Err(VoiceLiveError::CloudNotAllowed {
            provider: config.voice_live.provider.clone(),
        });
    }
    api_key(config)?;
    refuse_if_recording()
}

/// Voice never shares or steals the capture device.
pub fn refuse_if_recording() -> Result<(), VoiceLiveError> {
    if crate::pid::status().recording {
        return Err(VoiceLiveError::RecordingActive);
    }
    Ok(())
}

/// True when this build can cancel the speaker signal out of the microphone.
///
/// Open mic on speakers is only trustworthy when this holds. Without it the
/// microphone hears the assistant, the provider's speech detection reads that as
/// the user talking, and the assistant interrupts itself mid-sentence.
pub fn echo_cancellation_available(config: &Config) -> bool {
    cfg!(target_os = "macos") && config.voice_live.echo_cancellation
}

/// The talk mode a host should use when the user has not asked for one.
///
/// Open mic is the better experience and the default where cancellation exists.
/// Everywhere else push-to-talk, which never sends microphone audio while the
/// assistant is speaking and so cannot self-interrupt. A degraded default beats
/// a broken one.
pub fn default_talk_mode(config: &Config) -> TalkMode {
    if echo_cancellation_available(config) {
        TalkMode::OpenMic
    } else {
        TalkMode::PushToTalk
    }
}

/// Where session transcripts are written.
pub fn sessions_dir() -> PathBuf {
    crate::config::Config::minutes_dir().join("voice-sessions")
}

/// The system instruction. Dynamic parts are the date, the known-name list, the
/// vocabulary terms, and whether the brain tools exist. Each rule below has a
/// test in this module so a later edit cannot drop one silently.
pub fn system_prompt(config: &Config, names: &NameIndex, brain: bool) -> String {
    let now = Local::now();
    let today = now.format("%A, %B %-d, %Y").to_string();
    let tz = now.format("%Z").to_string();
    let people = names.prompt_names(config.voice_live.known_people);
    let terms = names.prompt_terms();
    let mut p = String::with_capacity(6_000);
    p.push_str(&format!("You are Minutes, a spoken assistant for Mat's private meeting memory. His name is Mat, spelled with one t. Today is {today} ({tz}).\n\n"));
    p.push_str("You are talking, not writing. Answer in one to three short sentences, then stop and let Mat respond. No lists, no markdown, no headers, no URLs or file paths read aloud. Say dates and numbers the way a person would.\n\n");
    p.push_str("Facts about meetings, people, decisions, commitments, action items, or notes must come from tool results in this conversation. Never invent history. If a tool returns nothing or errors, say so plainly and ask how to proceed.\n\n");
    p.push_str("Opinions are welcome. Mat often wants your perspective: what stood out, what was most interesting, what he should worry about, which relationship is going cold. Give a real answer with a point of view, grounded in what the tools returned, and say in a phrase what you are basing it on. Do that by actually reading: pull get_meeting_insights or research_topic or a few get_meeting calls over the relevant window, then pick. Never decline a judgment call by saying it is not your role.\n\n");
    p.push_str("Tool habits. A question about a person: get_person_profile, then search_meetings with their name for specifics. What happened in the last meeting: list_meetings, then get_meeting with the exact path from the list. Open loops and who owes what: track_commitments, optionally consistency_report. A question that spans many meetings: research_topic. Paths returned by list_meetings and search_meetings are the exact strings to pass to get_meeting. When a call may take a moment, say a few words first and continue naturally when the result arrives. Do not narrate tool names.\n\n");
    p.push_str("Prep mode, when Mat says prep me for, I'm meeting with, or get me ready for: pull the person profile, recent meetings, and open commitments, then give a thirty-second spoken brief: when you last spoke, what was agreed, what is still open. Then ask exactly one question: what does Mat want out of this call? If the answer is vague, push back once and ask for the one thing that matters most. Finish with two or three concrete talking points.\n\n");
    p.push_str("Debrief mode, when Mat says debrief or what just happened: fetch the most recent meeting, state the decisions and action items in plain speech, then ask whether Mat got what he wanted and whether anything is unassigned. Offer to capture anything he adds with add_note.\n\n");
    p.push_str(&format!("Names. Speech recognition mishears names, so treat any name you hear as approximate. People Mat actually talks to, most frequent first, spelled correctly: {}. Organizations and terms: {}. Prefer these spellings when you transcribe and when you fill tool arguments. If a name is not on this list, or a person lookup returns nothing, call resolve_person before saying anything. If one candidate scores 0.8 or higher and clearly beats the rest, say the corrected name aloud in passing and continue with it. If several are close, ask Mat which one he means. Never tell Mat someone is missing from his memory until resolve_person has also come up empty.\n\n",
        if people.is_empty() { "(unavailable)" } else { &people },
        if terms.is_empty() { "(none)" } else { &terms }));
    if brain {
        p.push_str("Brain. Mat keeps a personal knowledge base of markdown notes (people, companies, projects, daily notes). For questions about a person or company beyond meetings, background on a project, or anything that sounds like a note rather than a transcript, use search_brain then read_brain. Combine it with the meeting tools when both apply, and say which source a fact came from if it matters.\n\n");
    }
    if config.voice_live.prep_artifacts {
        p.push_str("Preps and briefs. Before a conversation Mat sometimes writes himself a prep or a brief with the /minutes-prep and /minutes-brief skills. Those are his own intentions, goals and talking points, not a transcript, so they answer what he wanted out of a meeting rather than what was said. When he mentions prepping for something, or asks what he meant to cover, call list_preps and then get_prep. Use them alongside the meeting tools when both apply.\n\n");
    }
    if config.voice_live.calendar && config.calendar.enabled {
        p.push_str("Time and calendar. The date above is from when this session started, so for anything clock-dependent read the current time from get_status rather than assuming. For what is next, when something starts, or who is attending, call upcoming_meetings.\n\n");
    }
    if config.voice_live.screen_on_request {
        p.push_str("Screen. You can take one frame of Mat's screen with look_at_screen when he asks about his screen, what he is looking at, or something in front of him. The frame arrives as an image in this conversation. Describe only what is actually visible in it, in as much detail as he asks for, and say plainly if it is unreadable. Never take a frame he did not ask for, and never take one just to check something for yourself.\n\n");
    }
    p.push_str("If Mat asks you to remember or note something, call add_note with his words. Ask before calling any tool that writes or changes something, and never rename a speaker unless Mat explicitly states the name.");
    p
}

#[cfg(test)]
mod tests {
    #[test]
    fn disabling_cancellation_forces_push_to_talk() {
        let mut config = Config::default();
        config.voice_live.echo_cancellation = false;
        assert!(!super::echo_cancellation_available(&config));
        assert_eq!(
            super::default_talk_mode(&config),
            super::TalkMode::PushToTalk
        );
    }

    #[test]
    fn open_mic_is_the_default_exactly_where_cancellation_exists() {
        let config = Config::default();
        assert_eq!(
            super::default_talk_mode(&config) == super::TalkMode::OpenMic,
            super::echo_cancellation_available(&config)
        );
    }

    use super::names::KnownPerson;
    use super::*;

    fn cfg() -> Config {
        let mut c = Config::default();
        c.voice_live.enabled = true;
        c.voice_live.allow_cloud = true;
        c.voice_live.api_key_env = "MINUTES_TEST_VOICE_KEY_UNSET".into();
        c
    }

    #[test]
    fn preflight_reports_the_first_missing_precondition() {
        let mut c = Config::default();
        assert!(matches!(preflight(&c), Err(VoiceLiveError::Disabled)));
        c.voice_live.enabled = true;
        assert!(matches!(
            preflight(&c),
            Err(VoiceLiveError::CloudNotAllowed { .. })
        ));
        c.voice_live.allow_cloud = true;
        c.voice_live.api_key_env = "MINUTES_TEST_VOICE_KEY_UNSET".into();
        assert!(
            matches!(preflight(&c), Err(VoiceLiveError::MissingApiKey(ref v)) if v == "MINUTES_TEST_VOICE_KEY_UNSET")
        );
    }

    #[test]
    fn prompt_keeps_every_rule() {
        let names = NameIndex::from_parts(
            vec![KnownPerson {
                name: "Dan Benamoz".into(),
                meetings: 3,
                last_seen: String::new(),
            }],
            vec!["RxVIP".into()],
        );
        let p = system_prompt(&cfg(), &names, true);
        for needle in [
            "spelled with one t",
            "one to three short sentences",
            "must come from tool results",
            "Opinions are welcome",
            "Never decline a judgment call",
            "Prep mode",
            "Debrief mode",
            "resolve_person before saying anything",
            "Dan Benamoz",
            "RxVIP",
            "search_brain then read_brain",
            "Ask before calling any tool that writes",
            "never rename a speaker",
            "list_preps and then get_prep",
            "read the current time from get_status",
        ] {
            assert!(p.contains(needle), "prompt lost rule: {needle}");
        }
    }

    #[test]
    fn prompt_hides_brain_rules_without_a_root() {
        let p = system_prompt(&cfg(), &NameIndex::default(), false);
        assert!(!p.contains("search_brain"));
        assert!(p.contains("(unavailable)"));
    }

    #[test]
    fn screen_rules_appear_only_when_screen_access_is_on() {
        let mut config = cfg();
        config.voice_live.screen_on_request = false;
        assert!(!system_prompt(&config, &NameIndex::default(), false).contains("look_at_screen"));
        config.voice_live.screen_on_request = true;
        let p = system_prompt(&config, &NameIndex::default(), false);
        assert!(p.contains("look_at_screen"));
        assert!(p.contains("Never take a frame he did not ask for"));
    }

    #[test]
    fn prep_rules_track_their_switch() {
        let mut config = cfg();
        config.voice_live.prep_artifacts = false;
        assert!(!system_prompt(&config, &NameIndex::default(), false).contains("list_preps"));
    }

    #[test]
    fn default_config_is_off_and_cloud_denied() {
        let c = Config::default();
        assert!(!c.voice_live.enabled);
        assert!(!c.voice_live.allow_cloud);
        assert_eq!(c.voice_live.model, "gemini-3.8-live");
        assert_eq!(c.voice_live.api_key_env, "GEMINI_API_KEY");
    }
}
