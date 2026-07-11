# How to remove AI notetaker bots from your meetings

Last reviewed: 2026-07-11

Bot notetakers join meetings the way a person does: they read a connected calendar, find the link, and dial in as a participant. There are exactly three levers — the calendar connection, the vendor's auto-join setting, and the meeting platform's participant controls.

## Removing your own bot

**Otter (OtterPilot / Otter Notetaker)**
- Stop it joining everything: Otter settings → turn off auto-join for calendar events, or disconnect Google/Microsoft calendar entirely
- Eject from a live meeting: open the participant list in Zoom/Meet/Teams and remove it like any attendee
- Otter's docs: https://help.otter.ai/hc/en-us/articles/12906714508823-Stop-Otter-Notetaker-from-automatically-joining-your-meetings and https://help.otter.ai/hc/en-us/articles/14288936562199-Remove-Otter-Notetaker-from-your-meeting-Zoom-Google-Meet-or-Microsoft-Teams

**Fireflies (Fred)**
- Fireflies settings → autojoin rules (invite-only or off), or disconnect the calendar
- Remove the notetaker from the participant list mid-call
- Fireflies' guide: https://guide.fireflies.ai/articles/7098191513-how-to-remove-fireflies-from-a-meeting-or-stop-it-from-joining

**Anything else** — same pattern: sever the calendar connection, turn off the auto-join rule. No calendar access, no way to find your meetings.

## Blocking other people's bots

You cannot disable a colleague's bot from your side. As host you can:

- Enable the waiting room/lobby and admit only humans (bots appear as guests named "Otter Notetaker", "Fireflies.ai Notetaker", etc.)
- Require signed-in participants
- Remove the bot from the participant list (Zoom can bar rejoining)
- Just ask — "please drop the notetaker for this one" is normal etiquette now
- Org-level: tenant policies restricting which apps/guest domains can join

## The version of this problem that solves itself

Every step above manages a symptom. The bot exists because cloud notetakers need your meeting audio on their servers, and joining as a fake participant is how they get it. Capture audio on the participant's own device instead and the category disappears: nothing joins the call, nothing to admit or eject.

That's how Minutes works — device-side recording, local transcription (whisper.cpp), markdown on your own disk. No bot and no cloud. (Granola is also botless, though it transcribes in the cloud.) One thing device-side capture doesn't change: tell people you're recording. The bot's one virtue was announcing itself; without it, consent is on you — where it belonged anyway.

## Related

- How botless capture works: https://useminutes.app/security
- Compare notetakers: https://useminutes.app/compare
