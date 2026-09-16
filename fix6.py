def rep1(s,a,b,l):
    n=s.count(a); assert n==1, f"{l}: got {n}"
    return s.replace(a,b)

# A. screen.rs: allow a wider frame than the recording pipeline uses
p="crates/core/src/screen.rs"; s=open(p).read()
s=rep1(s,'''/// Public so on-request surfaces (Voice Live's `look_at_screen`) can take one
/// frame without starting an interval capture session.
pub fn capture_screenshot(path: &Path) -> std::io::Result<()> {''','''/// Public so on-request surfaces (Voice Live's `look_at_screen`) can take one
/// frame without starting an interval capture session.
pub fn capture_screenshot(path: &Path) -> std::io::Result<()> {
    capture_screenshot_at_width(path, TARGET_WIDTH)
}

/// Capture one screenshot downscaled to `width` pixels across.
///
/// The recording pipeline wants many small frames. A single on-request frame
/// can afford more detail, and needs it: at [`TARGET_WIDTH`] a mouse pointer on
/// a Retina display is a couple of pixels and cannot be located reliably.
pub fn capture_screenshot_at_width(path: &Path, width: u32) -> std::io::Result<()> {''',"width fn")
s=rep1(s,'''                "--resampleWidth",
                &TARGET_WIDTH.to_string(),''','''                "--resampleWidth",
                &width.to_string(),''',"sips width")
s=rep1(s,'''    #[cfg(target_os = "linux")]
    {
        let result = crate::engine_process::command("scrot").arg(path).output();''','''    #[cfg(target_os = "linux")]
    {
        let _ = width;
        let result = crate::engine_process::command("scrot").arg(path).output();''',"linux unused")
s=rep1(s,'''    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        return Err(std::io::Error::other(
            "screen capture not supported on this platform",
        ));
    }''','''    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = width;
        return Err(std::io::Error::other(
            "screen capture not supported on this platform",
        ));
    }''',"other unused")
open(p,"w").write(s)

# B. protocol: the frame becomes the turn the model answers, not a side message
p="crates/core/src/voice_live/protocol.rs"; s=open(p).read()
s=rep1(s,'''/// The wire message for one image frame. Split out so the shape is testable
/// without a socket.
fn client_image_json(bytes: &[u8], mime: &str) -> Value {
    json!({ "clientContent": {
        "turns": [{ "role": "user", "parts": [{ "inlineData": {
            "mimeType": mime,
            "data": B64.encode(bytes),
        }}]}],
        "turnComplete": false,
    }})
}''','''/// The wire message for one image frame. Split out so the shape is testable
/// without a socket.
///
/// `turnComplete` is true and the frame carries a caption, so this is the turn
/// the model answers. An earlier version sent the frame as an open-ended
/// message just before the tool response, and the model reliably answered the
/// tool call without it and only saw the frame on the following turn. Waiting
/// longer did not help, because the frame was not part of the turn being
/// generated at all.
fn client_image_json(bytes: &[u8], mime: &str, caption: &str) -> Value {
    json!({ "clientContent": {
        "turns": [{ "role": "user", "parts": [
            { "inlineData": { "mimeType": mime, "data": B64.encode(bytes) } },
            { "text": caption },
        ]}],
        "turnComplete": true,
    }})
}''',"image json")
s=rep1(s,'''    pub fn send_image(&self, bytes: &[u8], mime: &str) -> Result<(), VoiceLiveError> {
        self.send_json(client_image_json(bytes, mime))
    }''','''    pub fn send_image(&self, bytes: &[u8], mime: &str, caption: &str) -> Result<(), VoiceLiveError> {
        self.send_json(client_image_json(bytes, mime, caption))
    }''',"send_image")
s=rep1(s,'''        let msg = client_image_json(&[1u8, 2, 3], "image/png");
        // Ordered channel, so the frame is in context before the tool response.
        assert!(msg.get("realtimeInput").is_none());
        let part = &msg["clientContent"]["turns"][0]["parts"][0]["inlineData"];
        assert_eq!(part["mimeType"], "image/png");
        assert!(part["data"].as_str().is_some_and(|d| !d.is_empty()));
        // The tool response completes the turn, not the frame.
        assert_eq!(msg["clientContent"]["turnComplete"], false);''','''        let msg = client_image_json(&[1u8, 2, 3], "image/png", "look at this");
        // Not the media side channel: that arrived a turn late every time.
        assert!(msg.get("realtimeInput").is_none());
        let parts = &msg["clientContent"]["turns"][0]["parts"];
        assert_eq!(parts[0]["inlineData"]["mimeType"], "image/png");
        assert!(parts[0]["inlineData"]["data"]
            .as_str()
            .is_some_and(|d| !d.is_empty()));
        assert_eq!(parts[1]["text"], "look at this");
        // The frame completes the turn, so it is what the model answers.
        assert_eq!(msg["clientContent"]["turnComplete"], true);''',"image test")
open(p,"w").write(s)

# C. session: silent tool result, then the frame as the answering turn
p="crates/core/src/voice_live/session.rs"; s=open(p).read()
s=rep1(s,'''                        // Media first: the frame must be in context before the
                        // text that tells the model to describe it, and the
                        // model unblocks on that text rather than on the frame.
                        if let Some(image) = &outcome.image {
                            if client.send_image(image, "image/png").is_err() {
                                break;
                            }
                            if settle > Duration::ZERO {
                                std::thread::sleep(settle);
                            }
                        }
                        if client
                            .send_tool_response(&call, &outcome.text, &scheduling)
                            .is_err()
                        {
                            break;
                        }''','''                        // A frame answers as a turn, not as a tool result.
                        // Close the call silently so it produces no speech of
                        // its own, then send the frame as the turn the model
                        // actually answers.
                        let media = outcome.image.is_some();
                        let this_scheduling = if media { "SILENT" } else { &scheduling };
                        if client
                            .send_tool_response(&call, &outcome.text, this_scheduling)
                            .is_err()
                        {
                            break;
                        }
                        if let Some(image) = &outcome.image {
                            if settle > Duration::ZERO {
                                std::thread::sleep(settle);
                            }
                            if client.send_image(image, "image/png", SCREEN_CAPTION).is_err() {
                                break;
                            }
                        }''',"worker")
s=rep1(s,"enum Control {",'''/// Sent with a screen frame, as the user turn the model answers.
const SCREEN_CAPTION: &str = "This is my screen at this exact moment, captured for the look_at_screen you just ran. Answer my question from this image and nothing else. If I dispute what you report, look at the image again and tell me what is actually there, even if that means disagreeing with me.";

enum Control {''',"caption")
open(p,"w").write(s)

# D. tools: back to non-blocking (SILENT pairs with it), wider frame
p="crates/core/src/voice_live/tools.rs"; s=open(p).read()
s=rep1(s,'''            d.push(decl_blocking(
                "look_at_screen",''','''            // Non-blocking on purpose: the frame is delivered as its own turn
            // and answered there, so the tool result itself is closed silently.
            d.push(decl(
                "look_at_screen",''',"decl")
s=rep1(s,'''/// A tool the model must wait for before it answers.
///
/// Non-blocking is right for reads whose answer is still true a second later.
/// It is wrong for anything about this instant: the model issues the call and
/// answers immediately from what it already had, so a question about the screen
/// gets described from the previous frame and the assistant runs a turn behind.
fn decl_blocking(name: &str, description: &str, properties: Value) -> Value {
    decl_json(
        name,
        description,
        json!({"type": "object", "properties": properties}),
        "BLOCKING",
    )
}

''',"")
s=rep1(s,"        if let Err(e) = crate::screen::capture_screenshot(&path) {",
"        // Wider than the recording pipeline's frames: at that size a pointer is\n        // a few pixels and cannot be located.\n        if let Err(e) = crate::screen::capture_screenshot_at_width(&path, SCREEN_FRAME_WIDTH) {","capture")
s=rep1(s,"/// A spoken reason a calendar read failed.","/// Width of an on-request screen frame, in pixels.\nconst SCREEN_FRAME_WIDTH: u32 = 1920;\n\n/// A spoken reason a calendar read failed.","const")
s=rep1(s,'''    fn the_screen_tool_blocks_so_the_answer_is_not_a_frame_behind() {
        let mut config = Config::default();
        config.voice_live.screen_on_request = true;
        let ctx = ToolContext::new(config, Arc::new(NameIndex::default()));
        let screen = ctx
            .declarations()
            .into_iter()
            .find(|d| d["name"] == "look_at_screen")
            .expect("look_at_screen should be declared");
        assert_eq!(screen["behavior"], "BLOCKING");
    }''','''    fn the_screen_tool_is_declared_and_answered_as_its_own_turn() {
        let mut config = Config::default();
        config.voice_live.screen_on_request = true;
        let ctx = ToolContext::new(config, Arc::new(NameIndex::default()));
        let screen = ctx
            .declarations()
            .into_iter()
            .find(|d| d["name"] == "look_at_screen")
            .expect("look_at_screen should be declared");
        // The frame answers as a turn, so the call itself is closed silently.
        assert_eq!(screen["behavior"], "NON_BLOCKING");
    }''',"test")
open(p,"w").write(s)

# E. settle is now only spacing between two ordered messages
p="crates/core/src/config.rs"; s=open(p).read()
s=rep1(s,"""    /// Pause between delivering a screen frame and releasing the tool result
    /// that describes it. `BLOCKING` makes the model wait for the result, not
    /// for the frame, so without a beat here it can unblock and answer before
    /// the image is in context and then hedge about not seeing anything.
    pub screen_settle_ms: u64,""","""    /// Pause between closing the screen tool call and sending the frame that
    /// answers it. Only spacing between two ordered messages; the frame is the
    /// turn the model answers, so this does not need to be long.
    pub screen_settle_ms: u64,""","doc")
s=rep1(s,"            screen_settle_ms: 1_000,","            screen_settle_ms: 150,","default")
open(p,"w").write(s)

# F. do not fold to a correction you have not verified
p="crates/core/src/voice_live/mod.rs"; s=open(p).read()
s=rep1(s,"Never describe a screen you have not actually looked at.",
"When Mat says you got something on screen wrong, look at the frame again and tell him what is actually there. If you still see the same thing, say so and say where you are looking. Agreeing with his correction without checking is worse than being wrong once, because then neither of you knows what is on the screen. Fine detail like a pointer position is genuinely hard to read, so say when you are unsure rather than asserting. Never describe a screen you have not actually looked at.","sycophancy")
s=rep1(s,'        assert!(p.contains("Never describe a screen you have not actually looked at"));','''        assert!(p.contains("Never describe a screen you have not actually looked at"));
        assert!(p.contains("Agreeing with his correction without checking"));''',"test")
open(p,"w").write(s)
print("ok")
