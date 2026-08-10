# How to turn off built-in AI notetakers in Zoom and Teams

Last reviewed: 2026-08-10

Zoom AI Companion and Teams Copilot are a different problem from Otter or Fireflies when they run inside their own platform. There is no bot in the participant list to eject, because the feature lives in the service already hosting your call, and the controls sit in settings you may not own.

## Why this one is different

A third-party notetaker has to get into your meeting the way a person does, by dialing in as a participant. That is what makes it removable. Built-in AI is not in the list because it never joined; it runs inside the service already carrying your audio.

Most controls here are therefore settings rather than actions, and the setting is often owned by someone else. There is one real live action, below: a Zoom host can stop Meeting Summary during the call.

One qualification that cuts against the framing above: Zoom now supports bringing AI Companion into meetings hosted on Google Meet and Microsoft Teams, and in that configuration it does join as a participant and can be removed like one. The "nothing to eject" rule holds for a platform's own AI inside its own meetings, not for Zoom's AI visiting someone else's.

## Zoom AI Companion

Sign in to the Zoom web portal, then take the path matching your scope:

- **Whole account:** Admin Center → Settings
- **One group:** Admin Center → Users → Groups → select the group → General configuration → Edit product settings
- **Just you:** Settings

In any of those, open **Zoom AI** and toggle the meeting summary features off. Admins changing an account or group can also click the lock icon, which prevents users below them turning it back on.

**If the toggle is grayed out**, it has been locked above you. Zoom's documentation is explicit: if a feature is grayed out, it has been locked at the account or group level and must be changed by an admin. This catches account owners too, who reasonably expect their own settings page to be authoritative. A setting locked at group level still reads as unavailable in your personal settings, so change it at the level where the lock was applied, not the level where you noticed it.

**Stopping it during the meeting.** This is the control most guides skip, and the only one that works in the moment. A Zoom host can open Zoom AI during the call and stop Meeting Summary, or stop all Zoom AI use, with the option to delete the associated meeting assets. It matters because of how the summary is scoped: Zoom documents that Meeting Summary covers the meeting from the point it was enabled, so stopping it partway limits the summary to what came before.

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
- **Speak up even mid-call.** Zoom says Meeting Summary covers the meeting from the point it was enabled, so stopping it partway genuinely limits the summary to what came before. Raising it late is not futile.
- **Escalate to whoever owns the tenant.** For a recurring concern, the durable fix is a policy change, not a per-meeting request repeated forever.
- **Move the conversation.** For genuinely confidential discussion, holding it somewhere you have confirmed the AI features are off is cleaner than fighting settings you do not control. Confirm rather than assume; what is enabled depends on how your tenant is configured.

## A note on what this page is not

We make a local notetaker, so it is worth being direct about where it does and does not belong here.

Everything above is governed by your organization. When an admin disables Copilot or locks Zoom AI, that is a decision your employer is entitled to make, often for legal or records-retention reasons you cannot see from your seat. **Running your own capture tool is not the workaround for that**, and we are not suggesting it. If your organization has decided meetings are not recorded, a local recording is still a recording, and your employer's policy still governs it.

Where Minutes is a reasonable answer is the case where notes are wanted and permitted, and the only open question is where they live. It records device-side, transcribes locally with whisper.cpp, and writes markdown to a folder you control, which some organizations prefer for confidential work precisely because the record stays inside a perimeter they already govern. That is an architecture to propose to whoever owns the policy, not a way around them.

To be exact about our own limits: conversation content leaves your machine when you send it somewhere, meaning a summarizer you configured, or an AI agent you connect and ask to read your meetings, whose provider then receives what it reads. Out of the box neither is happening.

And the constant: tell people you are recording, and follow your organization's policy on it.

## Sources

- Zoom: enabling/disabling Zoom AI meeting summary: https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0057623
- Zoom: stopping AI Companion during a meeting: https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0058013
- Zoom: AI Companion in Google Meet and Microsoft Teams meetings: https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0080354
- Microsoft Learn, Copilot in Teams meetings and events: https://learn.microsoft.com/en-us/microsoftteams/copilot-teams-transcription
- Microsoft Learn, transcription and captions for Teams meetings: https://learn.microsoft.com/en-us/microsoftteams/meeting-transcription-captions
- Microsoft Learn, recording and transcription for sensitive meetings: https://learn.microsoft.com/en-us/microsoftteams/manage-meeting-recording-options

Admin interfaces in both products change often. Verify the current path in your own tenant, and confirm the effect on a test meeting rather than assuming a saved setting took effect.

## Related

- Third-party notetaker bots (Otter, Fireflies): https://useminutes.app/resources/remove-ai-notetaker-bots-from-meetings
- Recording consent law by state: https://useminutes.app/resources/is-it-legal-to-record-a-meeting
- How botless capture works: https://useminutes.app/security
