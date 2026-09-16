def rep1(s,a,b,l):
    n=s.count(a); assert n==1, f"{l}: got {n}"
    return s.replace(a,b)

p="crates/core/src/config.rs"; s=open(p).read()
s=rep1(s,"""    /// Directory the relayed agent starts in. Empty uses the Minutes process
    /// directory, which for a desktop launch is not where any code lives.
    pub delegate_cwd: String,""","""    /// Directory the relayed agent starts in. Empty uses the Minutes process
    /// directory, which for a desktop launch is not where any code lives.
    pub delegate_cwd: String,
    /// Let the relayed agent change things: write files, open issues, call a
    /// service that writes. Off by default.
    ///
    /// The caller here is a cloud speech model deciding on its own when to
    /// relay, from audio it may have misheard, with nobody reviewing the
    /// request. RFC 0007 keeps phase 1 to a single write, `add_note`, for that
    /// reason. Turning this on is a deliberate widening of that boundary.
    pub delegate_writes: bool,""","field")
s=rep1(s,"            delegate_cwd: String::new(),","            delegate_cwd: String::new(),\n            delegate_writes: false,","default")
open(p,"w").write(s)

p="crates/core/src/voice_live/tools.rs"; s=open(p).read()
s=rep1(s,'''                     asks you to. Reply in under 120 words of plain prose, no markdown and no \\
                     code blocks, because it will be read aloud.\\n\\nQuestion: {question}"
                );''','''                     code blocks, because it will be read aloud.\\n\\nQuestion: {question}"
                );''',"trim")
s=rep1(s,'''                     when you could not find something rather than searching on. Answer only: \\
                     do not create, edit or delete anything unless the question explicitly \\
                     asks you to. Reply in under 120 words of plain prose, no markdown and no \\''','''                     when you could not find something rather than searching on. {rule} \\
                     Reply in under 120 words of plain prose, no markdown and no \\''',"rule slot")
s=rep1(s,'''                let prompt = format!(
                    "You are answering one question relayed from a voice assistant, and \\''','''                let rule = if cfg.voice_live.delegate_writes {
                    "You may change things when the question plainly asks you to, but say \\
                     exactly what you changed."
                } else {
                    "Answer only. Never create, edit, delete or send anything, and never \\
                     call a tool that writes, even if the question asks you to. If it does, \\
                     say that writing is turned off and stop."
                };
                let prompt = format!(
                    "You are answering one question relayed from a voice assistant, and \\''',"rule")
s=rep1(s,'&format!("Relay one question to Mat\'s local {agent_label} agent, which can read his code, files and connected services. Use it for anything outside meeting memory: his codebase, a repository, a document, or a system like a CRM or issue tracker. Ask one self-contained question, including any context from this conversation the agent would need, because it cannot hear you. It takes several seconds, so say you are checking before you call it."),',
'''&format!(
                        "Relay one question to Mat's local {agent_label} agent, which can read his code, files and connected services. Use it for anything outside meeting memory: his codebase, a repository, a document, or a system like a CRM or issue tracker. Ask one self-contained question, including any context from this conversation the agent would need, because it cannot hear you. It takes several seconds, so say you are checking before you call it.{}",
                        if self.config.voice_live.delegate_writes {
                            " It can also change things, so confirm with Mat out loud before asking it to."
                        } else {
                            " It only reads. If Mat wants something created, edited or sent, tell him writing through the agent is turned off rather than trying."
                        }
                    ),''',"decl")
s=rep1(s,"    fn delegation_inherits_the_assistant_launch_flags() {",'''    fn relayed_writes_are_off_until_deliberately_turned_on() {
        let config = Config::default();
        assert!(
            !config.voice_live.delegate_writes,
            "a speech model must not reach a write channel by default"
        );
        let mut on = Config::default();
        on.voice_live.ask_agent = true;
        on.voice_live.delegate_writes = true;
        let described = |c: Config| {
            ToolContext::new(c, Arc::new(NameIndex::default()))
                .declarations()
                .into_iter()
                .find(|d| d["name"] == "ask_agent")
                .map(|d| d["description"].as_str().unwrap_or_default().to_string())
        };
        if let Some(text) = described(Config::default()) {
            assert!(text.contains("only reads"));
        }
        if let Some(text) = described(on) {
            assert!(text.contains("confirm with Mat out loud"));
        }
    }

    #[test]
    fn delegation_inherits_the_assistant_launch_flags() {''',"test")
open(p,"w").write(s)

p="docs/architecture/config.md"; s=open(p).read()
s=rep1(s,'| `delegate_cwd` | `""` | Directory the relayed agent starts in. Empty uses the Minutes process directory, which for a desktop launch is not where any code lives |',
'''| `delegate_cwd` | `""` | Directory the relayed agent starts in. Empty uses the Minutes process directory, which for a desktop launch is not where any code lives |
| `delegate_writes` | `false` | Let the relayed agent change things. Off by default: the caller is a cloud speech model deciding on its own when to relay, from audio it may have misheard, with nobody reviewing the request. RFC 0007 holds phase 1 to a single write, `add_note`, for that reason. Narrowing the agent's own tool access through `delegate_agent_args` is the stronger control, since this one only instructs |''',"doc")
open(p,"w").write(s)

p="docs/configuration.md"; s=open(p).read()
s=rep1(s,'# delegate_cwd = "~/Sites"        # Where the relayed agent starts looking\n',
'''# delegate_cwd = "~/Sites"        # Where the relayed agent starts looking
# delegate_writes = false         # Let the relayed agent change things. Off by default:
#                                 # a misheard sentence should not open an issue or edit a file.
''',"doc2")
open(p,"w").write(s)
print("ok")
