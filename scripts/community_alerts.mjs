#!/usr/bin/env node
// Community alerts for the Minutes repository.
//
//   node scripts/community_alerts.mjs event      # one Slack message for the triggering event
//   node scripts/community_alerts.mjs sweep      # daily list of threads still waiting on a reply
//   node scripts/community_alerts.mjs --self-test
//
// Run by .github/workflows/community-alerts.yml. Posts to the Slack incoming
// webhook in SLACK_WEBHOOK_URL. Without it, the message is printed instead.
//
// Everything read from GitHub (titles, bodies, logins) is untrusted text. It is
// only escaped and quoted into a Slack message, never executed.

import { readFileSync, appendFileSync } from "node:fs";

const MAINTAINERS = new Set(
  (process.env.MINUTES_MAINTAINERS || "silverstein")
    .split(",")
    .map((login) => login.trim().toLowerCase())
    .filter(Boolean),
);
// The sweep skips threads whose last word is newer than this, because the
// event alert already covered them.
const SWEEP_MIN_AGE_HOURS = 20;
const SWEEP_MAX_ITEMS = 25;
const EXCERPT_CHARS = 280;

export function isBot(login, type) {
  if (!login) return false;
  return type === "Bot" || /\[bot\]$/i.test(login);
}

export function needsAlert(login, type) {
  if (!login) return true; // deleted account ("ghost"): still a person waiting
  return !MAINTAINERS.has(login.toLowerCase()) && !isBot(login, type);
}

export function escapeSlack(text) {
  return String(text ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

export function excerpt(text, limit = EXCERPT_CHARS) {
  const flat = String(text ?? "")
    .replace(/<!--[\s\S]*?-->/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  if (flat.length <= limit) return flat;
  return `${flat.slice(0, limit - 1).trimEnd()}…`;
}

function link(url, label) {
  return `<${url}|${escapeSlack(label).replace(/\|/g, "¦")}>`;
}

// Turn one webhook payload into { login, type, text }, or null when the event
// is not one we alert on.
export function describeEvent(name, event) {
  const action = event.action;
  let kind;
  let user;
  let url;
  let title;
  let body;
  if (name === "discussion" && action === "created") {
    kind = "New discussion";
    user = event.discussion.user;
    url = event.discussion.html_url;
    title = `#${event.discussion.number} ${event.discussion.title}`;
    body = event.discussion.body;
  } else if (name === "discussion_comment" && action === "created") {
    kind = event.comment.parent_id ? "Discussion reply" : "Discussion comment";
    user = event.comment.user;
    url = event.comment.html_url;
    title = `#${event.discussion.number} ${event.discussion.title}`;
    body = event.comment.body;
  } else if (name === "issues" && action === "opened") {
    kind = "New issue";
    user = event.issue.user;
    url = event.issue.html_url;
    title = `#${event.issue.number} ${event.issue.title}`;
    body = event.issue.body;
  } else if (name === "issue_comment" && action === "created") {
    kind = event.issue.pull_request ? "PR comment" : "Issue comment";
    user = event.comment.user;
    url = event.comment.html_url;
    title = `#${event.issue.number} ${event.issue.title}`;
    body = event.comment.body;
  } else if (
    name === "pull_request_target" &&
    ["opened", "reopened", "ready_for_review"].includes(action)
  ) {
    kind = action === "opened" ? "New pull request" : `Pull request ${action.replace(/_/g, " ")}`;
    user = event.pull_request.user;
    url = event.pull_request.html_url;
    title = `#${event.pull_request.number} ${event.pull_request.title}`;
    body = event.pull_request.body;
  } else {
    return null;
  }
  const login = user?.login ?? null;
  const type = user?.type ?? null;
  const quoted = excerpt(body);
  const lines = [
    `*${kind}* from ${escapeSlack(login ?? "ghost")}: ${link(url, title)}`,
  ];
  if (quoted) lines.push(`> ${escapeSlack(quoted)}`);
  return { login, type, text: lines.join("\n") };
}

// The most recent thing a person or bot did on a thread.
function latest(events) {
  return events
    .filter((entry) => entry && entry.at)
    .reduce((best, entry) => (!best || entry.at > best.at ? entry : best), null);
}

function actor(node) {
  return { login: node?.login ?? null, type: node?.__typename ?? null };
}

export function lastWordOnDiscussion(discussion) {
  const events = [{ ...actor(discussion.author), at: discussion.createdAt }];
  for (const comment of discussion.comments?.nodes ?? []) {
    events.push({ ...actor(comment.author), at: comment.createdAt });
    for (const reply of comment.replies?.nodes ?? []) {
      events.push({ ...actor(reply.author), at: reply.createdAt });
    }
  }
  return latest(events);
}

export function lastWordOnIssue(issue) {
  const events = [{ ...actor(issue.author), at: issue.createdAt }];
  for (const comment of issue.comments?.nodes ?? []) {
    events.push({ ...actor(comment.author), at: comment.createdAt });
  }
  return latest(events);
}

export function lastWordOnPullRequest(pull) {
  const events = [{ ...actor(pull.author), at: pull.createdAt }];
  for (const comment of pull.comments?.nodes ?? []) {
    events.push({ ...actor(comment.author), at: comment.createdAt });
  }
  for (const review of pull.reviews?.nodes ?? []) {
    events.push({ ...actor(review.author), at: review.submittedAt });
  }
  for (const commit of pull.commits?.nodes ?? []) {
    // A push after a review is the author asking for another look.
    events.push({ ...actor(pull.author), at: commit.commit?.committedDate });
  }
  return latest(events);
}

function age(at, now) {
  const hours = Math.max(0, (now - Date.parse(at)) / 3_600_000);
  if (hours < 48) return `${Math.round(hours)}h`;
  return `${Math.round(hours / 24)}d`;
}

export function waitingThreads(repository, now = Date.now()) {
  const waiting = [];
  const consider = (label, node, last) => {
    if (!last || !needsAlert(last.login, last.type)) return;
    if (now - Date.parse(last.at) < SWEEP_MIN_AGE_HOURS * 3_600_000) return;
    waiting.push({ label, node, last });
  };
  for (const discussion of repository.discussions?.nodes ?? []) {
    // A marked answer means the question is settled; a later follow-up still
    // gets its own event alert.
    if (discussion.closed || discussion.isAnswered) continue;
    consider("Discussion", discussion, lastWordOnDiscussion(discussion));
  }
  for (const issue of repository.issues?.nodes ?? []) {
    consider("Issue", issue, lastWordOnIssue(issue));
  }
  for (const pull of repository.pullRequests?.nodes ?? []) {
    if (isBot(pull.author?.login, pull.author?.__typename)) continue;
    consider(pull.isDraft ? "Draft PR" : "PR", pull, lastWordOnPullRequest(pull));
  }
  waiting.sort((a, b) => (a.last.at < b.last.at ? -1 : 1));
  return waiting;
}

export function sweepMessage(waiting, now = Date.now()) {
  if (waiting.length === 0) return null;
  const shown = waiting.slice(0, SWEEP_MAX_ITEMS);
  const lines = [
    `*${waiting.length} community thread${waiting.length === 1 ? "" : "s"} waiting on a reply* (oldest first)`,
  ];
  for (const { label, node, last } of shown) {
    lines.push(
      `• ${label} ${link(node.url, `#${node.number} ${node.title}`)}: ${escapeSlack(last.login ?? "ghost")}, ${age(last.at, now)} ago`,
    );
  }
  if (waiting.length > shown.length) {
    lines.push(`…and ${waiting.length - shown.length} more.`);
  }
  return lines.join("\n");
}

const SWEEP_QUERY = `
query($owner: String!, $name: String!) {
  repository(owner: $owner, name: $name) {
    discussions(first: 50, orderBy: {field: UPDATED_AT, direction: DESC}) {
      nodes {
        number title url closed isAnswered createdAt
        author { login __typename }
        comments(last: 20) {
          nodes {
            createdAt author { login __typename }
            replies(last: 1) { nodes { createdAt author { login __typename } } }
          }
        }
      }
    }
    issues(first: 50, states: OPEN, orderBy: {field: UPDATED_AT, direction: DESC}) {
      nodes {
        number title url createdAt
        author { login __typename }
        comments(last: 1) { nodes { createdAt author { login __typename } } }
      }
    }
    pullRequests(first: 50, states: OPEN, orderBy: {field: UPDATED_AT, direction: DESC}) {
      nodes {
        number title url isDraft createdAt
        author { login __typename }
        comments(last: 1) { nodes { createdAt author { login __typename } } }
        reviews(last: 1) { nodes { submittedAt author { login __typename } } }
        commits(last: 1) { nodes { commit { committedDate } } }
      }
    }
  }
}`;

async function fetchRepository() {
  const token = process.env.GITHUB_TOKEN;
  const [owner, name] = (process.env.GITHUB_REPOSITORY || "silverstein/minutes").split("/");
  if (!token) throw new Error("GITHUB_TOKEN is required for the sweep");
  const response = await fetch("https://api.github.com/graphql", {
    method: "POST",
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
      "user-agent": "minutes-community-alerts",
    },
    body: JSON.stringify({ query: SWEEP_QUERY, variables: { owner, name } }),
  });
  const payload = await response.json();
  if (!response.ok || payload.errors) {
    throw new Error(`GitHub GraphQL failed: ${response.status} ${JSON.stringify(payload.errors ?? payload)}`);
  }
  return payload.data.repository;
}

async function post(text) {
  const summary = process.env.GITHUB_STEP_SUMMARY;
  if (summary) appendFileSync(summary, `${text}\n`);
  const webhook = process.env.SLACK_WEBHOOK_URL;
  if (!webhook) {
    console.log("SLACK_WEBHOOK_URL is not set; would have posted:\n" + text);
    return;
  }
  const response = await fetch(webhook, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ text, unfurl_links: false, unfurl_media: false }),
  });
  if (!response.ok) {
    throw new Error(`Slack webhook failed: ${response.status} ${await response.text()}`);
  }
  console.log("Posted to Slack.");
}

function selfTest() {
  const assert = (condition, message) => {
    if (!condition) throw new Error(`self-test failed: ${message}`);
  };
  assert(!needsAlert("silverstein", "User"), "maintainer is skipped");
  assert(!needsAlert("SilverStein", "User"), "maintainer match ignores case");
  assert(!needsAlert("dependabot[bot]", "Bot"), "bot is skipped");
  assert(!needsAlert("dependabot", "Bot"), "GraphQL bot is skipped");
  assert(needsAlert("contributor-b", "User"), "contributor alerts");
  assert(needsAlert(null, null), "ghost alerts");
  assert(escapeSlack("<@here> & <!channel>") === "&lt;@here&gt; &amp; &lt;!channel&gt;", "escapes mentions");
  assert(excerpt("a\n\n b <!-- template --> c") === "a b c", "excerpt flattens and drops comments");
  assert(excerpt("x".repeat(400)).length === EXCERPT_CHARS, "excerpt truncates");

  const reply = describeEvent("discussion_comment", {
    action: "created",
    discussion: { number: 101, title: "Ideas <b>" },
    comment: {
      parent_id: 7,
      user: { login: "contributor-a", type: "User" },
      html_url: "https://github.com/silverstein/minutes/discussions/101#c",
      body: "Thanks!\n\nWill do.",
    },
  });
  assert(reply.login === "contributor-a", "reply login");
  assert(reply.text.startsWith("*Discussion reply* from contributor-a: <https://"), "reply heading");
  assert(reply.text.includes("#101 Ideas &lt;b&gt;>"), "reply title escaped");
  assert(reply.text.endsWith("> Thanks! Will do."), "reply excerpt");
  assert(describeEvent("issues", { action: "closed", issue: {} }) === null, "ignores other actions");
  const pr = describeEvent("issue_comment", {
    action: "created",
    issue: { number: 202, title: "t", pull_request: {} },
    comment: { user: { login: "a", type: "User" }, html_url: "https://x", body: "" },
  });
  assert(pr.text.startsWith("*PR comment*") && !pr.text.includes("\n"), "PR comment without body");

  const now = Date.parse("2026-11-25T12:00:00Z");
  const repository = {
    discussions: {
      nodes: [
        {
          number: 1, title: "answered", url: "u1", closed: false, createdAt: "2026-11-01T00:00:00Z",
          author: { login: "x", __typename: "User" },
          comments: { nodes: [{ createdAt: "2026-11-02T00:00:00Z", author: { login: "x" },
            replies: { nodes: [{ createdAt: "2026-11-03T00:00:00Z", author: { login: "silverstein" } }] } }] },
        },
        {
          number: 2, title: "waiting", url: "u2", closed: false, createdAt: "2026-11-18T00:00:00Z",
          author: { login: "contributor-a", __typename: "User" }, comments: { nodes: [] },
        },
        {
          number: 3, title: "fresh", url: "u3", closed: false, createdAt: "2026-11-25T10:00:00Z",
          author: { login: "y", __typename: "User" }, comments: { nodes: [] },
        },
        {
          number: 5, title: "answered with thanks", url: "u7", closed: false, isAnswered: true, createdAt: "2026-11-01T00:00:00Z",
          author: { login: "q", __typename: "User" }, comments: { nodes: [] },
        },
        {
          number: 4, title: "closed", url: "u4", closed: true, createdAt: "2026-11-01T00:00:00Z",
          author: { login: "z", __typename: "User" }, comments: { nodes: [] },
        },
      ],
    },
    issues: { nodes: [] },
    pullRequests: {
      nodes: [
        {
          number: 202, title: "pushed after review", url: "u5", isDraft: false, createdAt: "2026-11-20T00:00:00Z",
          author: { login: "contributor-b", __typename: "User" }, comments: { nodes: [] },
          reviews: { nodes: [{ submittedAt: "2026-11-21T00:00:00Z", author: { login: "silverstein" } }] },
          commits: { nodes: [{ commit: { committedDate: "2026-11-22T00:00:00Z" } }] },
        },
        {
          number: 303, title: "bump", url: "u6", isDraft: false, createdAt: "2026-11-01T00:00:00Z",
          author: { login: "dependabot", __typename: "Bot" }, comments: { nodes: [] },
          reviews: { nodes: [] }, commits: { nodes: [] },
        },
      ],
    },
  };
  const waiting = waitingThreads(repository, now);
  assert(waiting.map((w) => w.node.number).join(",") === "2,202", `waiting set was ${waiting.map((w) => w.node.number)}`);
  const message = sweepMessage(waiting, now);
  assert(message.startsWith("*2 community threads waiting on a reply*"), "sweep heading");
  assert(message.includes("• Discussion <u2|#2 waiting>: contributor-a, 8d ago"), "sweep line");
  assert(sweepMessage([], now) === null, "empty sweep posts nothing");
  console.log("community_alerts self-test passed");
}

async function main() {
  const mode = process.argv[2];
  if (mode === "--self-test") return selfTest();
  if (mode === "event") {
    const name = process.env.GITHUB_EVENT_NAME;
    const event = JSON.parse(readFileSync(process.env.GITHUB_EVENT_PATH, "utf8"));
    const described = describeEvent(name, event);
    if (!described) return console.log(`No alert for ${name}/${event.action}.`);
    if (!needsAlert(described.login, described.type)) {
      return console.log(`No alert: ${described.login} is a maintainer or bot.`);
    }
    return post(described.text);
  }
  if (mode === "sweep") {
    const message = sweepMessage(waitingThreads(await fetchRepository()));
    if (!message) return console.log("Nothing is waiting on a reply.");
    return post(message);
  }
  throw new Error("usage: community_alerts.mjs event|sweep|--self-test");
}

await main();
