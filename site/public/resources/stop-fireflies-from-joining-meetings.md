# How to stop Fireflies from joining your meetings

Last reviewed: 2026-08-09

There is a documented deadline on this one, and it is three minutes.

## The three-minute rule

Fireflies' documentation states that if the notetaker is removed **before 3 minutes elapse**, no transcript or notes will be created.

Worth knowing before a call rather than during one, because it turns a vague annoyance into a deadline you can act on.

Read the claim precisely, though. It is a statement about *transcript and notes creation*. Fireflies does not document what happens to audio captured during those first minutes, and does not say the bot recorded nothing. Treat the window as a reliable way to avoid a transcript existing, not as a guarantee that nothing was captured. If that distinction matters for your meeting, keep the bot out rather than race it.

## Remove it now

The bot appears as an ordinary participant.

- **Zoom** — participant list → **More** next to Fireflies Notetaker → **Remove**
- **Google Meet** — participant list → three-dot menu next to Fireflies → **Remove from the call**
- **Microsoft Teams** — participant list → ellipsis next to Fireflies → **Remove from meeting**

These are participant controls, which in practice means host or co-host. Fireflies' documentation does not state whether a non-host can remove the notetaker, and publishes no chat command for it. If you are not the host, asking the organizer is the reliable path rather than assuming a shortcut exists.

## Stop it coming back

**Turn it off entirely:** Settings → **Recording & Privacy** → Recording → toggle off **Auto-record meetings**. Per Fireflies, that stops the notetaker joining future meetings scheduled on your connected calendar.

**Or keep it, on invitation only:** from the homepage **Upcoming** button, open **Calendar meeting settings** under Fireflies Notetaker. The default covers every meeting with a web-conference link; switch it to **"Only when I invite fred@fireflies.ai"**. This is usually the setting people actually wanted.

**If it still joins, check your recording rules.** Fireflies documents rules targeting meetings by keyword, participant email address, or domain, and states they take precedence over your auto-join setting. The direction that surprises people is the affirmative one: a recording rule can pull Fred into a meeting even when auto-record is set to manual only. A rule created months ago can keep producing transcripts long after you believe you switched auto-join off.

Auto-join is the first place everyone looks, and recording rules are the second place the answer usually is.

## When the bot is not yours

Fireflies can reach a meeting through someone else entirely. A Fireflies user with a connected Google or Outlook calendar adds `fred@fireflies.ai` to the invite, and the bot then attempts to join subject to that user's settings and the meeting platform's admission controls. Other attendees need no Fireflies account and were never asked.

The part worth being straight about: every Fireflies setting above governs **your own notetaker**. Your dashboard cannot cancel a bot that another person's account scheduled, even when the meeting is on your calendar. For someone else's Fred the real options are three:

- Remove it from the call, if you are host or co-host
- Ask the person who invited it to cancel it (one click on their side)
- Remove `fred@fireflies.ai` from the calendar invite, if it is an invite you can edit

For an organization, the durable control is not a Fireflies setting at all. Fireflies rules only govern the account they belong to. Blocking external attendees' bots is a job for your meeting platform's admission and tenant policies.

## The version of this problem that solves itself

Everything above manages a symptom. The bot exists because cloud notetakers need your meeting audio on their servers, and sending a synthetic participant is how they collect it. Capture on the participant's own device instead and the category evaporates: nothing joins, nothing in the participant list, no three-minute deadline to race.

In fairness, Fireflies now offers bot-free capture of its own via a Google Meet SDK integration and a desktop app. Read it precisely: those remove the visible bot from the call, not the upload. Fireflies' own SDK documentation describes meeting audio and video being shared with Fireflies and processed into its notebook. No bot in the participant list is a courtesy improvement, not an architectural one.

Minutes is the architectural version: device-side recording, local transcription (whisper.cpp), markdown on your own disk, so no bot joins and no audio is uploaded. To be exact about our own limits, transcript text leaves your machine only if you explicitly configure a provider-backed summarizer, which is off by default and documented on our security page.

One thing device-side capture does not change: tell people you are recording. The bot's single virtue was announcing itself; without it, consent is your job, which is where it belonged anyway.

## Sources

- Remove Fireflies from a meeting: https://guide.fireflies.ai/articles/7098191513-how-to-remove-fireflies-from-a-meeting-or-stop-it-from-joining
- Disable auto-join settings: https://guide.fireflies.ai/articles/8587670572-how-to-disable-the-fireflies-auto-join-settings
- Recording rules: https://guide.fireflies.ai/articles/3115936908-how-to-use-recording-rules-to-record-or-skip-specific-meetings
- Invite Fireflies to meetings: https://guide.fireflies.ai/articles/4335268657-how-to-invite-fireflies-to-meetings
- Google Meet SDK bot-free recording: https://guide.fireflies.ai/articles/3309351579-integrate-google-meet-sdk-with-fireflies-for-bot-free-meeting-recording

## Related

- Fireflies vs Minutes: https://useminutes.app/compare/fireflies-vs-minutes
- Other notetakers and platforms: https://useminutes.app/resources/remove-ai-notetaker-bots-from-meetings
- Recording consent law by state: https://useminutes.app/resources/is-it-legal-to-record-a-meeting
- How botless capture works: https://useminutes.app/security
