def rep1(s,a,b,l):
    n=s.count(a); assert n==1, f"{l}: got {n}"
    return s.replace(a,b)

# config
p="crates/core/src/config.rs"; s=open(p).read()
s=rep1(s,"""    /// Let the relayed agent change things: write files, open issues, call a""","""    /// Labs toy: let the assistant generate and play music steered by what it
    /// knows about a conversation. Off by default and deliberately separate
    /// from the memory features.
    pub music: bool,
    /// Music model id.
    pub music_model: String,
    /// Longest piece to play, in seconds.
    pub music_max_secs: u64,
    /// Let the relayed agent change things: write files, open issues, call a""","fields")
s=rep1(s,"            delegate_writes: false,","            delegate_writes: false,\n            music: false,\n            music_model: \"lyria-3.5\".into(),\n            music_max_secs: 45,","defaults")
open(p,"w").write(s)

# module
p="crates/core/src/voice_live/mod.rs"; s=open(p).read()
s=rep1(s,"pub mod mcp;","pub mod mcp;\npub mod music;","mod")
s=rep1(s,'    if config.voice_live.screen_on_request {','''    if config.voice_live.music {
        p.push_str("Music. You can make and play a short instrumental piece with make_music. When Mat asks for music for a meeting, a call or a moment, first read what you actually know about it, from upcoming_meetings, a prep, or the meeting itself, then write the brief yourself and say in a sentence what you drew on. Describe instruments, tempo and mood; it is instrumental, so do not ask for lyrics or singing. It takes most of a minute, so say you are writing something first. Never while a recording is running, and do not offer it in the middle of real work.\\n\\n");
    }
    if config.voice_live.screen_on_request {''',"prompt")
s=rep1(s,'            "call ask_agent with one self-contained question",','''            "call ask_agent with one self-contained question",''',"noop")
open(p,"w").write(s)

# tool outcome carries audio, and the tool itself
p="crates/core/src/voice_live/tools.rs"; s=open(p).read()
s=rep1(s,"""    /// An image the host must push into the session before delivering `text`.
    /// Tool results are text, so a frame reaches the model as session media and
    /// the text only tells it the frame is there.
    pub image: Option<Vec<u8>>,
}""","""    /// An image the host must push into the session before delivering `text`.
    /// Tool results are text, so a frame reaches the model as session media and
    /// the text only tells it the frame is there.
    pub image: Option<Vec<u8>>,
    /// Audio for the host to play: PCM16 mono at the provider's rate. Not sent
    /// to the model, which has no reason to listen to it.
    pub audio: Option<Vec<u8>>,
}""","outcome")
n=s.count("            image: None,\n        }")
assert n>=1, n
s=s.replace("            image: None,\n        }","            image: None,\n            audio: None,\n        }")
s=rep1(s,"""                    is_error: false,
                    elapsed: started.elapsed(),
                    image: Some(bytes),
                }""","""                    is_error: false,
                    elapsed: started.elapsed(),
                    image: Some(bytes),
                    audio: None,
                }""","image outcome")
s=rep1(s,"""        if name == "look_at_screen" {
            return self.look_at_screen(started);
        }""","""        if name == "look_at_screen" {
            return self.look_at_screen(started);
        }
        if name == "make_music" {
            return self.make_music(args, started);
        }""","route")
s=rep1(s,"""    /// Capture one frame of the screen. Handled outside `dispatch` because it
    /// is the only tool that answers with media rather than text.""","""    /// Render a piece of music. Outside `dispatch` because it answers with
    /// audio for the host to play rather than with text for the model.
    fn make_music(&self, args: &Value, started: Instant) -> ToolOutcome {
        let fail = |msg: String| ToolOutcome {
            text: json!({ "error": msg }).to_string(),
            is_error: true,
            elapsed: started.elapsed(),
            image: None,
            audio: None,
        };
        if !self.config.voice_live.music {
            return fail("music is off; set [voice_live] music = true in config.toml".into());
        }
        // Music reaches the microphone and then the transcript. Capture is
        // never degraded by an optional consumer.
        if crate::pid::status().recording {
            return fail(
                "a recording is running, so music stays off until it stops".into(),
            );
        }
        let Some(description) = str_arg(args, "description") else {
            return fail("description is required".into());
        };
        match super::music::compose(&self.config, &description) {
            Ok(piece) => ToolOutcome {
                text: json!({
                    "playing": true,
                    "seconds": piece.seconds.round() as i64,
                    "saved_to": piece.path.display().to_string(),
                    "structure": piece.structure,
                    "note": "The piece is playing now. Say one short sentence about what you made and what you based it on, then stop talking and let it play.",
                })
                .to_string(),
                is_error: false,
                elapsed: started.elapsed(),
                image: None,
                audio: Some(piece.pcm16),
            },
            Err(e) => fail(e),
        }
    }

    /// Capture one frame of the screen. Handled outside `dispatch` because it
    /// is the only tool that answers with media rather than text.""","make_music")
s=rep1(s,"""        if self.config.voice_live.screen_on_request {
            d.push(decl(""","""        if self.config.voice_live.music {
            d.push(decl(
                "make_music",
                "Generate and play a short instrumental piece. Write the brief yourself from what you know about the conversation in question: instruments, tempo, mood, and what it is for. Instrumental only, so never ask for lyrics or singing. Takes most of a minute, and will refuse while a recording is running.",
                json!({"description": {"type": "string", "description": "What the music should sound like, in a sentence or two"}}),
            ));
        }
        if self.config.voice_live.screen_on_request {
            d.push(decl(""","music decl")
s=rep1(s,"    fn relayed_writes_are_off_until_deliberately_turned_on() {","""    fn music_is_refused_while_the_switch_is_off() {
        let ctx = ToolContext::new(Config::default(), Arc::new(NameIndex::default()));
        let out = ctx.execute("make_music", &json!({"description": "something warm"}));
        assert!(out.is_error);
        assert!(out.audio.is_none());
        assert!(out.text.contains("music is off"));
    }

    #[test]
    fn music_is_not_declared_until_it_is_turned_on() {
        let names: Vec<String> = ToolContext::new(Config::default(), Arc::new(NameIndex::default()))
            .declarations()
            .into_iter()
            .map(|d| d["name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(!names.contains(&"make_music".to_string()));
    }

    #[test]
    fn relayed_writes_are_off_until_deliberately_turned_on() {""","tests")
open(p,"w").write(s)

# session plays it
p="crates/core/src/voice_live/session.rs"; s=open(p).read()
s=rep1(s,"""                        let media = outcome.image.is_some();""","""                        if let Some(pcm) = &outcome.audio {
                            audio_out.send(pcm.clone()).ok();
                        }
                        let media = outcome.image.is_some();""","play")
s=rep1(s,"        let (tool_tx, tool_rx) = bounded::<FunctionCall>(64);","        let (tool_tx, tool_rx) = bounded::<FunctionCall>(64);\n        let (audio_out, audio_in) = unbounded::<Vec<u8>>();","channel")
s=rep1(s,"            let log_tx = self.log.as_ref().map(|l| l.sender());","            let log_tx = self.log.as_ref().map(|l| l.sender());\n            let audio_out = audio_out.clone();","clone")
s=rep1(s,"""                recv(self.control_rx) -> ctl => {""","""                // Audio a tool produced, queued for the speaker on this thread
                // because the playback handle lives here.
                recv(audio_in) -> pcm => {
                    if let Ok(pcm) = pcm {
                        self.audio.push_pcm16(&pcm);
                    }
                }
                recv(self.control_rx) -> ctl => {""","recv")
open(p,"w").write(s)
print("ok")
