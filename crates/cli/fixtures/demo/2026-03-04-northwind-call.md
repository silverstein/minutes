---
title: Northwind Customer Call — SSO Friction
type: meeting
minutes_demo: true
date: 2026-03-04T15:30:00-07:00
duration: 31m
attendees: [Mat S., Riley B.]
people: [mat, riley]
tags: [customer, northwind, onboarding, retention]
decisions:
  - text: Ship SSO nested-groups fix for Northwind-shaped accounts before end of Q1 (2026-03-31)
    topic: sso
    authority: high
action_items:
  - assignee: mat
    task: Fix SSO nested-groups onboarding for Northwind
    due: 2026-03-31
    status: open
    promised_to: riley
speaker_map:
  - speaker_label: SPEAKER_0
    name: mat
    confidence: high
    source: manual
  - speaker_label: SPEAKER_1
    name: riley
    confidence: high
    source: manual
---

## Summary

- Riley (Northwind, Director of Ops) frustrated. SSO nested-groups failing for their Okta setup.
- Mat personally committed to ship the fix before 2026-03-31.
- Riley used the word "churn" twice unprompted.

## Transcript

[SPEAKER_1 0:00] Half of our eighteen people bounced at SSO this month. I can't keep selling this.
[SPEAKER_0 0:22] What's the failure point?
[SPEAKER_1 0:29] Okta nested groups. Your product expects flat users.
[SPEAKER_0 0:49] End of the quarter. March 31. That's a personal commitment from me.
[SPEAKER_1 1:02] If it slips, we're going to have a different conversation in Q2.
