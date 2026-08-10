# How to turn off built-in AI notetakers in Zoom and Teams

Last reviewed: 2026-08-10

Zoom AI Companion and Teams Copilot are a different problem from Otter or Fireflies. Those arrive as participants you can eject. These are features of the platform hosting your call, so there is nothing in the participant list to remove, and the controls live in settings you may not own.

## Why this one is different

A third-party notetaker has to get into your meeting the way a person does, by dialing in as a participant. That is what makes it removable. Built-in AI is not in the list because it never joined; it runs inside the service already carrying your audio.

Every control here is therefore a setting rather than an action, and the setting is often owned by someone else. If you are not an admin, the most reliable move in the room is the social one: ask the organizer to turn it off, and confirm they did.

## Zoom AI Companion

Sign in to the Zoom web portal, then take the path matching your scope:

- **Whole account:** Admin Center → Settings
- **One group:** Admin Center → Users → Groups, then select the group
- **Just you:** Settings

In any of those, open **Zoom AI** and toggle the meeting summary features off. Admins changing an account or group can also click the lock icon, which prevents users below them turning it back on.

**If the toggle is grayed out**, it has been locked above you. Zoom's documentation is explicit: if a feature is grayed out, it has been locked at the account or group level and must be changed by an admin. This catches account owners too, who reasonably expect their own settings page to be authoritative. A setting locked at group level still reads as unavailable in your personal settings, so change it at the level where the lock was applied, not the level where you noticed it.

## Microsoft Teams Copilot

In the Teams admin center: **Meetings → Meeting policies → Recording & transcription → Copilot**. The dropdown offers four options:

- On
- On with saved transcript required
- On with transcript saved by default
- Off

Assign the policy to the users or groups it should apply to.

**"Off" is a default, not an enforcement.** From Microsoft's own documentation: the only Copilot policy setting you can enforce is "On with saved transcript required." The others create a default that your organizers can change.

So an admin who selects Off has expressed a preference, not a guarantee, and an organizer can still switch Copilot on for their own meeting. If your compliance posture assumes the tenant setting is binding, test that assumption before relying on it; enforcement of the outcome you want may need to come from policy and training rather than the dropdown.

Because Copilot builds on the transcript, review transcription policy at the same time rather than assuming it matches the Copilot setting.

## What if you are not the admin

- **Ask the organizer.** They hold the per-meeting controls in both platforms.
- **Say it before the substance.** Summaries are generated from the whole meeting, so raising it late is worse than raising it early.
- **Escalate to whoever owns the tenant.** For a recurring concern, the durable fix is a policy change, not a per-meeting request repeated forever.
- **Move the conversation.** For genuinely confidential discussion, a platform whose vendor is not summarizing by default is cleaner than fighting settings you do not control.

## The version of this problem that solves itself

Notice what the above has in common: the notes are produced by the company hosting your call, using settings owned by someone in your organization, and your ability to say no depends on where you sit in an admin hierarchy. That is a governance arrangement, not a product feature you can toggle away.

The alternative is to let the platform run the call and keep the record yourself. Minutes records device-side, transcribes locally with whisper.cpp, and writes markdown to your own disk, so the notes exist because you made them rather than because a tenant policy allowed it. It does not disable anything the platform is doing, and it is not a way around your organization's rules. It changes who holds the copy you rely on.

To be exact about our own limits: transcript text leaves your machine only if you deliberately configure a provider-backed summarizer, which is off by default.

One thing device-side capture does not change: tell people you are recording, and follow your organization's policy on it.

## Sources

- Zoom: enabling/disabling Zoom AI meeting summary: https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0057623
- Microsoft Learn, Copilot in Teams meetings and events: https://learn.microsoft.com/en-us/microsoftteams/copilot-teams-transcription
- Microsoft Learn, transcription and captions for Teams meetings: https://learn.microsoft.com/en-us/microsoftteams/meeting-transcription-captions
- Microsoft Learn, recording and transcription for sensitive meetings: https://learn.microsoft.com/en-us/microsoftteams/manage-meeting-recording-options

Admin interfaces in both products change often. Verify the current path in your own tenant, and confirm the effect on a test meeting rather than assuming a saved setting took effect.

## Related

- Third-party notetaker bots (Otter, Fireflies): https://useminutes.app/resources/remove-ai-notetaker-bots-from-meetings
- Recording consent law by state: https://useminutes.app/resources/is-it-legal-to-record-a-meeting
- How botless capture works: https://useminutes.app/security
