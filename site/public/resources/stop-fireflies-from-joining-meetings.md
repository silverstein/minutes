# How to stop Fireflies from joining your meetings

Last reviewed: 2026-08-09

There is a deadline on this one. Get Fred out fast enough and Fireflies keeps nothing at all.

## The three-minute rule

Fireflies' own documentation states that if the notetaker is removed **before 3 minutes elapse**, no transcript or notes will be created.

That makes the first three minutes qualitatively different from every minute after. Eject Fred inside the window and there is nothing to delete, nothing to request, nothing sitting in someone else's workspace. Miss it and a transcript exists; removing the bot then only stops it capturing more.

Worth knowing before the meeting rather than during it. If a call turns sensitive in the first minute, you have a real deadline and it is short.

## Remove it now

The bot appears as an ordinary participant.

- **Zoom** — participant list → **More** next to Fireflies Notetaker → **Remove**
- **Google Meet** — participant list → three-dot menu next to Fireflies → **Remove from the call**
- **Microsoft Teams** — participant list → ellipsis next to Fireflies → **Remove from meeting**

These are host-side participant controls. Fireflies' documentation does not state whether a non-host can remove the notetaker, and publishes no chat command for it, so if you are not the host, asking the organizer is the reliable path rather than assuming a shortcut exists.

## Stop it coming back

**Turn it off entirely:** Settings → **Recording & Privacy** → Recording → toggle off **Auto-record meetings**. Per Fireflies, that stops the notetaker joining any future meeting scheduled on your connected calendar.

**Or keep it, on invitation only:** from the homepage **Upcoming** button, open **Calendar meeting settings** under Fireflies Notetaker. The default is every meeting with a web-conference link; switch it to **"Only when I invite fred@fireflies.ai"**. This is usually the setting people actually wanted.

**If it still joins, check your recording rules.** Fireflies supports rules targeting meetings by keyword, participant email address, or domain, and per its documentation those rules **override auto-join settings**. A rule set up months ago can keep pulling Fred into calls long after you believe you switched auto-join off. Auto-join is the first place people look and the second place the answer usually is.

## When the bot is not yours

Fireflies has a route into your meeting that does not require you to use Fireflies at all: it joins when `fred@fireflies.ai` is added as a guest on the calendar invite. Any organizer can do that. You do not need an account, and you were never asked.

If the meeting is on your own connected calendar, find the event under **Upcoming Meetings** in Fireflies and switch it off. If it is not your calendar, the options narrow to two honest ones: remove the bot in the call if you are host, or say something. "Could we do this one without the notetaker?" costs nothing and works immediately.

For an organization, the durable version is a recording rule scoped to a domain: the same mechanism that causes the surprise above, used deliberately.

## The version of this problem that solves itself

Everything above manages a symptom. The bot exists because cloud notetakers need your meeting audio on their servers, and sending a synthetic participant is how they collect it. Capture on your own device instead and the category evaporates: nothing joins, nothing in the participant list, no three-minute deadline.

In fairness, Fireflies now offers bot-free capture of its own via a Google Meet SDK integration and a desktop app. Read it precisely: those remove the visible bot from the call, not the upload. The audio still goes to Fireflies.

Minutes removes both: device-side recording, local transcription (whisper.cpp), markdown on your own disk. No bot and no vendor copy.

One thing device-side capture does not change: tell people you are recording. The bot's single virtue was announcing itself; without it, consent is your job, which is where it belonged anyway.

## Related

- Fireflies vs Minutes: https://useminutes.app/compare/fireflies-vs-minutes
- Other notetakers and platforms: https://useminutes.app/resources/remove-ai-notetaker-bots-from-meetings
- Recording consent law by state: https://useminutes.app/resources/is-it-legal-to-record-a-meeting
- How botless capture works: https://useminutes.app/security
