def rep1(s,a,b,l):
    n=s.count(a); assert n==1, f"{l}: got {n}"
    return s.replace(a,b)

# A. an unreadable calendar must never look like an empty one
p="crates/core/src/voice_live/tools.rs"; s=open(p).read()
s=rep1(s,'''            "upcoming_meetings" => {
                let minutes = int_arg(args, "within_minutes", 720).clamp(5, 10_080) as u32;''','''            "upcoming_meetings" => {
                // An empty list and an unreadable calendar look identical from
                // here, and reporting the second as the first tells Mat his day
                // is clear when it is not. Probe first; the probe never prompts.
                let access = crate::calendar::calendar_access_status();
                if !access.can_read() {
                    return Err(format!(
                        "cannot read the calendar ({}). Tell Mat his calendar is unreachable and never that it is empty.",
                        calendar_access_label(access)
                    ));
                }
                let minutes = int_arg(args, "within_minutes", 720).clamp(5, 10_080) as u32;''',"calendar guard")
s=rep1(s,'                Ok(json!({ "within_minutes": minutes, "events": events }))',
'                Ok(json!({ "within_minutes": minutes, "calendar_readable": true, "events": events }))',"calendar ok")
s=rep1(s,"/// Which agent CLI voice relays to:",'''/// A spoken reason a calendar read failed.
fn calendar_access_label(access: crate::calendar::CalendarAccess) -> &'static str {
    use crate::calendar::CalendarAccess;
    match access {
        CalendarAccess::FullAccess => "readable",
        CalendarAccess::WriteOnly => "Minutes has add-only access, not read access",
        CalendarAccess::Denied => "calendar access is denied in System Settings",
        CalendarAccess::Restricted => "calendar access is restricted by policy",
        CalendarAccess::NotDetermined => "calendar access has not been granted yet",
        CalendarAccess::Unknown => "the calendar helper did not answer",
    }
}

/// Which agent CLI voice relays to:''',"label fn")
s=rep1(s,'"note": "A frame of Mat\'s screen taken just now was added to this conversation. It replaces any earlier frame: his screen has probably changed since, so describe only this newest one and never answer from a previous one. Say so plainly if it is unreadable.",',
'"note": "A frame of Mat\'s screen taken just now was added to this conversation immediately before this result. It replaces any earlier frame: his screen has probably changed since, so describe only this newest one and never answer from a previous one. If you cannot actually see a new image, do not hedge or guess at what might be there. Say plainly that the frame did not arrive and call look_at_screen once more.",',"note")
s=rep1(s,"    fn the_screen_tool_blocks_so_the_answer_is_not_a_frame_behind() {",'''    fn an_unreadable_calendar_is_an_error_not_an_empty_day() {
        let mut config = Config::default();
        config.voice_live.calendar = true;
        config.calendar.enabled = true;
        let ctx = ToolContext::new(config, Arc::new(NameIndex::default()));
        let out = ctx.execute("upcoming_meetings", &json!({}));
        // On a machine with no calendar access this must fail loudly. Where it
        // does succeed the result says so explicitly instead of being a bare list.
        if out.is_error {
            assert!(out.text.contains("never that it is empty"));
        } else {
            assert!(out.text.contains("calendar_readable"));
        }
    }

    #[test]
    fn the_screen_tool_blocks_so_the_answer_is_not_a_frame_behind() {''',"calendar test")
open(p,"w").write(s)

# B. give the frame a moment to land before the model is unblocked
p="crates/core/src/config.rs"; s=open(p).read()
s=rep1(s,"""    /// How long to wait for that agent before giving up.
    pub delegate_timeout_secs: u64,
}""","""    /// How long to wait for that agent before giving up.
    pub delegate_timeout_secs: u64,
    /// Pause between delivering a screen frame and releasing the tool result
    /// that describes it. `BLOCKING` makes the model wait for the result, not
    /// for the frame, so without a beat here it can unblock and answer before
    /// the image is in context and then hedge about not seeing anything.
    pub screen_settle_ms: u64,
}""","config field")
s=rep1(s,"""            delegate_timeout_secs: 120,
        }""","""            delegate_timeout_secs: 120,
            screen_settle_ms: 400,
        }""","config default")
open(p,"w").write(s)

p="crates/core/src/voice_live/session.rs"; s=open(p).read()
s=rep1(s,'''                        // Media first: the frame must be in context before the
                        // text that tells the model to describe it.
                        if let Some(image) = &outcome.image {
                            if client.send_image(image, "image/png").is_err() {
                                break;
                            }
                        }''','''                        // Media first: the frame must be in context before the
                        // text that tells the model to describe it, and the
                        // model unblocks on that text rather than on the frame.
                        if let Some(image) = &outcome.image {
                            if client.send_image(image, "image/png").is_err() {
                                break;
                            }
                            if settle > Duration::ZERO {
                                std::thread::sleep(settle);
                            }
                        }''',"settle")
s=rep1(s,"            let scheduling = self.scheduling.clone();","            let scheduling = self.scheduling.clone();\n            let settle = Duration::from_millis(self.config.voice_live.screen_settle_ms.min(3_000));","settle var")
open(p,"w").write(s)

# C. prompt: say so and retry rather than hedging
p="crates/core/src/voice_live/mod.rs"; s=open(p).read()
s=rep1(s,"Never take a frame he did not ask for, and never take one just to check something for yourself.",
"If a frame does not reach you, say plainly that it did not arrive and take another rather than hedging about not making out details. Never take a frame he did not ask for, and never take one just to check something for yourself.","prompt retry")
s=rep1(s,'            assert!(p.contains("that is you, so do not describe it"));','''            assert!(p.contains("that is you, so do not describe it"));
        assert!(p.contains("say plainly that it did not arrive"));''',"test")
open(p,"w").write(s)
print("ok")
