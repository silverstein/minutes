---
title: Product Prioritization — Q2 Cut Lines
type: meeting
minutes_demo: true
date: 2026-04-17T13:00:00-07:00
duration: 58m
attendees: [Mat S., Alex K., Sam W., Jordan M.]
people: [mat, alex, sam, jordan]
tags: [product, prioritization, q2, roadmap]
decisions:
  - text: Kill the advanced analytics feature. It was on the Q2 roadmap; it is no longer.
    topic: advanced-analytics
    authority: high
  - text: Keep team billing as the primary Q2 engineering investment
    topic: team-billing
    authority: high
  - text: Ship a scoped reporting-exports fix in Q2. Date-ranged CSV export, no dashboard, no visualizations.
    topic: reporting-exports
    authority: high
action_items:
  - assignee: sam
    task: Remove advanced analytics mockups from the product roadmap page
    due: 2026-04-18
    status: open
  - assignee: alex
    task: Ship scoped reporting-exports (date-ranged CSV) by 2026-05-02
    due: 2026-05-02
    status: open
speaker_map:
  - speaker_label: SPEAKER_0
    name: mat
    confidence: high
    source: manual
  - speaker_label: SPEAKER_1
    name: alex
    confidence: high
    source: manual
---

## Summary

- Advanced analytics cut from Q2 roadmap.
- Team billing stays as primary Q2 investment.
- Reporting exports get a scoped green-light: date-ranged CSV only, two weeks of engineering, ship 2026-05-02.

## Transcript

[SPEAKER_0 0:00] Three things on the table. Advanced analytics, team billing, reporting exports.
[SPEAKER_1 0:08] Kill analytics. No customer pulling for it.
[SPEAKER_0 0:34] Killed. Team billing stays, on track for April 15. Reporting exports scoped: date-ranged CSV, no dashboard.
[SPEAKER_1 1:02] Two weeks if it's literally date filter + CSV.
[SPEAKER_0 1:14] Scope locked. Ship by May 2.
