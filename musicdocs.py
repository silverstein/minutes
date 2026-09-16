def rep1(s,a,b,l):
    n=s.count(a); assert n==1, f"{l}: got {n}"
    return s.replace(a,b)

p="crates/core/src/voice_live/music.rs"; s=open(p).read()
s=rep1(s,'''    let dir = music_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;''','''    let dir = music_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    // Owner-only, like every other directory Minutes writes.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }''',"dir perms")
open(p,"w").write(s)

p="docs/architecture/config.md"; s=open(p).read()
s=rep1(s,"| `delegate_writes` | `false` |","""| `music` | `false` | Labs toy. Generate and play a short instrumental piece steered by what the assistant knows about a conversation. Deliberately separate from the memory features and off by default. Refuses while a recording is running, because music through the speakers reaches the microphone and then the transcript |
| `music_model` | `"lyria-3.5"` | Music model id |
| `music_max_secs` | `45` | Longest stretch to play. The whole piece is still kept under `~/.minutes/music` |
| `delegate_writes` | `false` |""","arch")
open(p,"w").write(s)

p="docs/configuration.md"; s=open(p).read()
s=rep1(s,"# delegate_writes = false         # Let the relayed agent change things. Off by default:\n",
"""# music = true                    # Labs toy: generate and play short instrumental music
#                                 # steered by a meeting or a prep. Refuses while recording.
# music_max_secs = 45
# delegate_writes = false         # Let the relayed agent change things. Off by default:
""","config")
open(p,"w").write(s)

p="docs/rfcs/0007-voice-live.md"; s=open(p).read()
s=rep1(s,"## Open questions","""## Outside the phases

`music.rs` is a labs toy behind `[voice_live] music`, off by default and not part of any phase above. The assistant reads a meeting, a prep or the calendar, writes a music brief itself, and the module renders it through the provider's music model and hands back samples. It exists because Minutes is the only thing that knows both what your next conversation is and what you wrote about it beforehand. It refuses while a recording is running: music through the speakers reaches the microphone and then the transcript, and RFC 0004's rule that an optional consumer never degrades capture applies to it exactly as it does to everything else.

## Open questions""","rfc")
open(p,"w").write(s)
print("ok")
