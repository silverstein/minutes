export interface RoutingFixture {
  utterance: string;
  expectedSkill: string;
}

export const ROUTING_FIXTURES: RoutingFixture[] = [
  {
    utterance: "Can you brief me on Sarah before my next call?",
    expectedSkill: "minutes-brief",
  },
  {
    utterance: "Please clean up recordings from older meetings.",
    expectedSkill: "minutes-cleanup",
  },
  {
    utterance: "Start Coach for this meeting and help me land the next steps.",
    expectedSkill: "minutes-copilot",
  },
  {
    utterance: "Can you debrief that call for me?",
    expectedSkill: "minutes-debrief",
  },
  {
    utterance: "Show me everyone who mentioned Stripe across all meetings.",
    expectedSkill: "minutes-graph",
  },
  {
    utterance: "What ideas did I have while walking yesterday?",
    expectedSkill: "minutes-ideas",
  },
  {
    utterance: "Please backfill knowledge from my meetings into the wiki.",
    expectedSkill: "minutes-ingest",
  },
  {
    utterance: "Check for stale action items in my meetings.",
    expectedSkill: "minutes-lint",
  },
  {
    utterance: "Show my recent recordings from today.",
    expectedSkill: "minutes-list",
  },
  {
    utterance: "How did I do in my last meeting?",
    expectedSkill: "minutes-mirror",
  },
  {
    utterance: "Note that Alex wants monthly billing.",
    expectedSkill: "minutes-note",
  },
  {
    utterance: "Prep me for my call with Sarah tomorrow.",
    expectedSkill: "minutes-prep",
  },
  {
    utterance: "What happened in my meetings today?",
    expectedSkill: "minutes-recap",
  },
  {
    utterance: "Please start recording this meeting.",
    expectedSkill: "minutes-record",
  },
  {
    utterance: "What did we discuss about pricing?",
    expectedSkill: "minutes-search",
  },
  {
    utterance: "How do I start using Minutes on a new machine?",
    expectedSkill: "minutes-setup",
  },
  {
    utterance: "Mark that as a win.",
    expectedSkill: "minutes-tag",
  },
  {
    utterance: "Why isn't Minutes working right now?",
    expectedSkill: "minutes-verify",
  },
  {
    utterance: "Please review this walkthrough video and summarize it.",
    expectedSkill: "minutes-video-review",
  },
  {
    utterance: "Can you give me a weekly summary of what happened this week?",
    expectedSkill: "minutes-weekly",
  },
];
