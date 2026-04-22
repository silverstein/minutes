/**
 * Minutes Dashboard — MCP App
 *
 * Interactive meeting dashboard with detail views.
 * Uses ext-apps SDK for host communication.
 */

import {
  App,
  type McpUiHostContext,
  applyDocumentTheme,
  applyHostStyleVariables,
} from "@modelcontextprotocol/ext-apps";
import type { CallToolResult } from "@modelcontextprotocol/sdk/types.js";
import "./mcp-app.css";

// ─── DOM Helpers ──────────────────────────────────────────────────────────────

const $ = (id: string) => document.getElementById(id)!;

function escapeHtml(s: string): string {
  const el = document.createElement("span");
  el.textContent = s;
  return el.innerHTML;
}

function escapeAttr(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/'/g, "&#39;");
}

// ─── State ────────────────────────────────────────────────────────────────────

let cachedDashboardData: any = null;
let currentDetailData: { content: string; path: string; title: string } | null = null;
let activeFilter: "all" | "meeting" | "memo" = "all";
let recordingPollTimer: ReturnType<typeof setInterval> | null = null;

const VIEWS = ["loading", "error", "dashboard", "detail", "person", "people-map", "report"] as const;
type View = (typeof VIEWS)[number];

function showView(view: View) {
  for (const id of VIEWS) {
    $(id).style.display = id === view ? "" : "none";
  }
  // Defer size update until after browser reflow
  requestAnimationFrame(() => {
    app.sendSizeChanged({ height: document.documentElement.scrollHeight });
  });
}

function showError(message: string) {
  $("error-message").textContent = message;
  showView("error");
}

// ─── Markdown Rendering ──────────────────────────────────────────────────────

function parseFrontmatter(raw: string): { frontmatter: Record<string, any>; body: string } {
  const match = raw.match(/^---\n([\s\S]*?)\n---\n([\s\S]*)$/);
  if (!match) return { frontmatter: {}, body: raw };

  const yamlBlock = match[1];
  const body = match[2];
  const fm: Record<string, any> = {};

  // Simple YAML parser — handles key: value, arrays, and nested items
  let currentArray: any[] | null = null;
  let currentObj: Record<string, any> | null = null;

  for (const line of yamlBlock.split("\n")) {
    // Array item with object fields: "  - what: ..."
    const arrayObjMatch = line.match(/^\s{2,4}- (\w+):\s*(.*)$/);
    if (arrayObjMatch && currentArray !== null) {
      currentObj = { [arrayObjMatch[1]]: arrayObjMatch[2].trim() };
      currentArray.push(currentObj);
      continue;
    }

    // Continuation of object fields: "    who: ..."
    const contMatch = line.match(/^\s{4,}(\w+):\s*(.*)$/);
    if (contMatch && currentObj !== null) {
      currentObj[contMatch[1]] = contMatch[2].trim();
      continue;
    }

    // Inline array: "key: [a, b, c]"
    const inlineArrayMatch = line.match(/^(\w[\w_]*):\s*\[(.+)\]$/);
    if (inlineArrayMatch) {
      currentArray = null;
      currentObj = null;
      fm[inlineArrayMatch[1]] = inlineArrayMatch[2].split(",").map((s) => s.trim().replace(/^["']|["']$/g, ""));
      continue;
    }

    // Array item (simple): "  - value"
    const simpleArrayMatch = line.match(/^\s{2,4}-\s+(.+)$/);
    if (simpleArrayMatch && currentArray !== null) {
      currentObj = null;
      currentArray.push(simpleArrayMatch[1].trim());
      continue;
    }

    // Top-level key
    const kvMatch = line.match(/^(\w[\w_]*):\s*(.*)$/);
    if (kvMatch) {
      currentObj = null;
      const key = kvMatch[1];
      const value = kvMatch[2].trim();
      if (value === "" || value === "[]") {
        // Start of block array
        currentArray = [];
        fm[key] = currentArray;
      } else {
        currentArray = null;
        fm[key] = value.replace(/^["']|["']$/g, "");
      }
      continue;
    }
  }

  return { frontmatter: fm, body };
}

function renderMarkdown(md: string): string {
  let html = escapeHtml(md);

  // Headings
  html = html.replace(/^### (.+)$/gm, "<h3>$1</h3>");
  html = html.replace(/^## (.+)$/gm, "<h2>$1</h2>");

  // Bold and italic
  html = html.replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>");
  html = html.replace(/\*(.+?)\*/g, "<em>$1</em>");

  // Inline code
  html = html.replace(/`([^`]+)`/g, "<code>$1</code>");

  // Blockquotes (escaped &gt;)
  html = html.replace(/^&gt; (.+)$/gm, "<blockquote>$1</blockquote>");

  // List items
  html = html.replace(/^- (.+)$/gm, "<li>$1</li>");
  html = html.replace(/(<li>[\s\S]*?<\/li>)/g, "<ul>$1</ul>");
  // Collapse consecutive <ul> blocks
  html = html.replace(/<\/ul>\s*<ul>/g, "");

  // Paragraphs — wrap remaining loose text lines
  html = html.replace(/^(?!<[hublop])((?!<).+)$/gm, "<p>$1</p>");

  // Clean up empty paragraphs
  html = html.replace(/<p>\s*<\/p>/g, "");

  return html;
}

// ─── Safe DOM builders ────────────────────────────────────────────────────────
// All dynamic content is escaped via escapeHtml() above before insertion.
// Data originates from the trusted Minutes CLI binary on the local machine.

function setInner(el: HTMLElement, html: string) {
  el.innerHTML = html;
}

// ─── App ──────────────────────────────────────────────────────────────────────

const app = new App(
  { name: "Minutes Dashboard", version: "1.0.0" },
  {},
  { autoResize: true },
);

// ─── Feature: Recording Status ───────────────────────────────────────────────

let recordingStartTime: number | null = null;

async function checkRecordingStatus() {
  try {
    const result = await app.callServerTool({ name: "get_status", arguments: {} });
    const text = result.content?.map((c: any) => ("text" in c ? c.text : "")).join("") || "";
    const isRecording = text.includes("in progress") && !text.includes("No recording");

    const banner = $("recording-banner");
    if (isRecording) {
      if (!recordingStartTime) recordingStartTime = Date.now();
      banner.style.display = "flex";
      updateRecordingElapsed();
      if (!recordingPollTimer) {
        recordingPollTimer = setInterval(updateRecordingElapsed, 1000);
      }
    } else {
      banner.style.display = "none";
      recordingStartTime = null;
      if (recordingPollTimer) {
        clearInterval(recordingPollTimer);
        recordingPollTimer = null;
      }
    }
  } catch {
    // Non-fatal — just hide the banner
    $("recording-banner").style.display = "none";
  }
}

function updateRecordingElapsed() {
  if (!recordingStartTime) return;
  const elapsed = Math.floor((Date.now() - recordingStartTime) / 1000);
  const mins = Math.floor(elapsed / 60);
  const secs = elapsed % 60;
  $("rec-elapsed").textContent = `${mins}:${String(secs).padStart(2, "0")}`;
}

// Stop button
document.addEventListener("click", async (e) => {
  if ((e.target as HTMLElement).id === "rec-stop-btn") {
    const btn = e.target as HTMLButtonElement;
    btn.disabled = true;
    btn.textContent = "Stopping...";
    try {
      await app.callServerTool({ name: "stop_recording", arguments: {} });
      $("recording-banner").style.display = "none";
      recordingStartTime = null;
      if (recordingPollTimer) {
        clearInterval(recordingPollTimer);
        recordingPollTimer = null;
      }
    } catch {
      btn.disabled = false;
      btn.textContent = "Stop";
    }
  }
});

// ─── Feature: Client-side Filter ─────────────────────────────────────────────

function applyFilter() {
  const query = ($("filter-input") as HTMLInputElement).value.toLowerCase();
  document.querySelectorAll(".meeting-card").forEach((card) => {
    const el = card as HTMLElement;
    const title = el.querySelector(".meeting-title")?.textContent?.toLowerCase() || "";
    const date = el.querySelector(".meeting-date")?.textContent?.toLowerCase() || "";
    const type = el.querySelector(".badge")?.textContent?.toLowerCase() || "";

    const matchesQuery = !query || title.includes(query) || date.includes(query);
    const matchesType = activeFilter === "all" || type === activeFilter;

    el.style.display = matchesQuery && matchesType ? "" : "none";
  });
}

// Filter input
$("filter-input").addEventListener("input", applyFilter);

// Type toggles
document.querySelectorAll(".filter-btn").forEach((btn) => {
  btn.addEventListener("click", async () => {
    const filter = (btn as HTMLElement).dataset.filter;

    // People tab → load relationship map
    if (filter === "people") {
      document.querySelectorAll(".filter-btn").forEach((b) => b.classList.remove("active"));
      btn.classList.add("active");
      showView("loading");
      $("loading-text").textContent = "Loading people...";
      try {
        const result = await app.callServerTool({ name: "relationship_map", arguments: {} });
        handleToolResult(result);
      } catch (e: any) {
        showError(e.message || "Failed to load people");
      }
      return;
    }

    document.querySelectorAll(".filter-btn").forEach((b) => b.classList.remove("active"));
    btn.classList.add("active");
    activeFilter = filter as any;
    applyFilter();
  });
});

// ─── Feature: Context Injection ──────────────────────────────────────────────

function sendMeetingToContext() {
  if (!currentDetailData) return;
  const { title, content } = currentDetailData;
  const truncated = content.length > 15000 ? content.slice(0, 15000) + "\n\n[truncated]" : content;
  app.updateModelContext({
    content: [{ type: "text", text: `Meeting: ${title}\n\n${truncated}` }],
  });
}

// ─── Feature: Fullscreen ─────────────────────────────────────────────────────

let isFullscreen = false;

function toggleFullscreen() {
  isFullscreen = !isFullscreen;
  app.requestDisplayMode({ mode: isFullscreen ? "fullscreen" : "inline" });
  const btn = $("fullscreen-btn");
  if (btn) btn.textContent = isFullscreen ? "Exit Fullscreen" : "Fullscreen";
}

// ─── View: Dashboard ─────────────────────────────────────────────────────────

function renderDashboard(data: any) {
  const meetings: any[] = data.meetings || [];
  const actions: any[] = data.actions || [];
  cachedDashboardData = data;

  // Stats
  const memoCount = meetings.filter((m) => m.content_type === "memo").length;
  const meetingCount = meetings.length - memoCount;
  setInner(
    $("stats-bar"),
    `<div class="stat"><span class="stat-value">${meetings.length}</span><span class="stat-label">Total</span></div>
     <div class="stat"><span class="stat-value">${meetingCount}</span><span class="stat-label">Meetings</span></div>
     <div class="stat"><span class="stat-value">${memoCount}</span><span class="stat-label">Memos</span></div>
     ${actions.length ? `<div class="stat"><span class="stat-value">${actions.length}</span><span class="stat-label">Open Actions</span></div>` : ""}`,
  );

  // Meeting cards
  const listEl = $("meetings-list");
  setInner(
    listEl,
    meetings
      .map(
        (m) => `
    <div class="meeting-card" data-path="${escapeAttr(m.path || "")}">
      <div class="meeting-date">${escapeHtml(m.date || "")}</div>
      <div class="meeting-title">${escapeHtml(m.title || "Untitled")}</div>
      <div class="meeting-meta">
        <span class="badge badge-${m.content_type === "memo" ? "memo" : "meeting"}">${escapeHtml(m.content_type || "meeting")}</span>
        ${m.words ? `<span class="word-count">${escapeHtml(String(m.words))} words</span>` : ""}
        ${m.duration ? `<span class="duration">${escapeHtml(String(m.duration))}</span>` : ""}
      </div>
      ${m.snippet ? `<div class="meeting-snippet">${escapeHtml(m.snippet)}</div>` : ""}
    </div>`,
      )
      .join(""),
  );

  // Click → detail
  listEl.querySelectorAll(".meeting-card").forEach((card) => {
    card.addEventListener("click", async () => {
      const path = (card as HTMLElement).dataset.path;
      if (!path) return;
      showView("loading");
      $("loading-text").textContent = "Loading meeting...";
      try {
        const result = await app.callServerTool({ name: "get_meeting", arguments: { path } });
        handleToolResult(result);
      } catch (e: any) {
        showError(e.message || "Failed to load meeting");
      }
    });
  });

  // Actions sidebar
  const sidebarEl = $("actions-sidebar");
  if (actions.length > 0) {
    setInner(
      sidebarEl,
      `<div class="sidebar-title">Open Action Items</div>` +
        actions
          .map((a) => {
            const overdue = a.by_date && new Date(a.by_date) < new Date();
            return `
        <div class="intent-item action${overdue ? " overdue" : ""}">
          <div class="intent-what">${escapeHtml(a.what || "")}</div>
          ${a.who ? `<div class="intent-who">@${escapeHtml(a.who)}</div>` : ""}
          ${a.by_date ? `<div class="intent-date">by ${escapeHtml(a.by_date)}</div>` : ""}
          ${a.title ? `<div class="intent-source">${escapeHtml(a.title)}</div>` : ""}
        </div>`;
          })
          .join(""),
    );
  } else {
    setInner(sidebarEl, "");
  }

  // Reset filter UI
  ($("filter-input") as HTMLInputElement).value = "";
  activeFilter = "all";
  document.querySelectorAll(".filter-btn").forEach((b) => {
    b.classList.toggle("active", (b as HTMLElement).dataset.filter === "all");
  });

  showView("dashboard");

  // Check recording status
  checkRecordingStatus();
}

// ─── View: Detail ─────────────────────────────────────────────────────────────

function renderDetail(data: any) {
  const content: string = data.content || "";
  const meetingPath: string = data.path || "";
  const { frontmatter, body } = parseFrontmatter(content);

  // Track for context injection
  const title = frontmatter.title || meetingPath.split("/").pop()?.replace(/\.md$/, "") || "Meeting";
  currentDetailData = { content, path: meetingPath, title };

  // Header
  setInner(
    $("detail-header"),
    `<h1>${escapeHtml(title)}</h1>
    <div class="detail-meta">
      ${frontmatter.date ? `<span class="meta-item">${escapeHtml(frontmatter.date)}</span>` : ""}
      ${frontmatter.duration ? `<span class="meta-item">${escapeHtml(frontmatter.duration)}</span>` : ""}
      ${frontmatter.content_type ? `<span class="badge badge-${frontmatter.content_type === "memo" ? "memo" : "meeting"}">${escapeHtml(frontmatter.content_type)}</span>` : ""}
      ${frontmatter.words ? `<span class="meta-item">${escapeHtml(String(frontmatter.words))} words</span>` : ""}
    </div>
    ${
      Array.isArray(frontmatter.attendees) && frontmatter.attendees.length
        ? `<div class="attendees">Attendees: ${frontmatter.attendees.map((a: string) => `<span class="attendee clickable" data-person="${escapeAttr(a)}">${escapeHtml(a)}</span>`).join(" ")}</div>`
        : ""
    }
    ${
      Array.isArray(frontmatter.tags) && frontmatter.tags.length
        ? `<div class="tags">${frontmatter.tags.map((t: string) => `<span class="tag">${escapeHtml(t)}</span>`).join("")}</div>`
        : ""
    }`,
  );

  // Clickable attendees → person profile
  $("detail-header").querySelectorAll(".attendee.clickable").forEach((el) => {
    el.addEventListener("click", async (e) => {
      e.stopPropagation();
      const name = (el as HTMLElement).dataset.person;
      if (!name) return;
      showView("loading");
      $("loading-text").textContent = `Loading profile for ${name}...`;
      try {
        const result = await app.callServerTool({ name: "get_person_profile", arguments: { name } });
        handleToolResult(result);
      } catch (err: any) {
        showError(err.message || "Failed to load profile");
      }
    });
  });

  // Body
  setInner($("detail-content"), renderMarkdown(body));

  // Panels
  const panels: string[] = [];

  const actionItems = frontmatter.action_items;
  if (Array.isArray(actionItems) && actionItems.length > 0) {
    panels.push(`
      <div class="panel"><h3>Action Items</h3>
        ${actionItems
          .map(
            (item: any) => `
          <div class="intent-item action">
            <div class="intent-what">${escapeHtml(typeof item === "string" ? item : item.what || "")}</div>
            ${item.who ? `<div class="intent-who">@${escapeHtml(item.who)}</div>` : ""}
            ${item.by_date ? `<div class="intent-date">by ${escapeHtml(item.by_date)}</div>` : ""}
          </div>`,
          )
          .join("")}
      </div>`);
  }

  const decisions = frontmatter.decisions;
  if (Array.isArray(decisions) && decisions.length > 0) {
    panels.push(`
      <div class="panel"><h3>Decisions</h3>
        ${decisions
          .map(
            (d: any) => `
          <div class="intent-item decision">
            <div class="intent-what">${escapeHtml(typeof d === "string" ? d : d.what || "")}</div>
          </div>`,
          )
          .join("")}
      </div>`);
  }

  setInner($("detail-panels"), panels.join(""));

  // Toolbar buttons
  $("back-btn").onclick = () => {
    if (isFullscreen) toggleFullscreen();
    if (cachedDashboardData) {
      renderDashboard(cachedDashboardData);
    }
  };
  $("context-btn").onclick = sendMeetingToContext;
  $("fullscreen-btn").onclick = toggleFullscreen;
  $("fullscreen-btn").textContent = isFullscreen ? "Exit Fullscreen" : "Fullscreen";

  showView("detail");
}

// ─── View: Person ─────────────────────────────────────────────────────────────

function renderPerson(data: any) {
  const name: string = data.name || "Unknown";
  const topics: any[] = data.top_topics || [];
  const openIntents: any[] = data.open_intents || [];
  const recentMeetings: any[] = data.recent_meetings || [];

  // Header
  setInner(
    $("person-header"),
    `<h1>${escapeHtml(name)}</h1>
    <div class="person-stats">
      <div class="stat"><span class="stat-value">${recentMeetings.length}</span><span class="stat-label">Meetings</span></div>
      <div class="stat"><span class="stat-value">${openIntents.length}</span><span class="stat-label">Open Items</span></div>
      <div class="stat"><span class="stat-value">${topics.length}</span><span class="stat-label">Topics</span></div>
    </div>`,
  );

  // Body
  const sections: string[] = [];

  if (topics.length > 0) {
    sections.push(`
      <div class="person-section"><h3>Top Topics</h3>
        ${topics.map((t: any) => `<div class="topic-item"><span class="topic-name">${escapeHtml(t.topic || "")}</span><span class="topic-count">${t.count || 0}</span></div>`).join("")}
      </div>`);
  }

  if (openIntents.length > 0) {
    sections.push(`
      <div class="person-section"><h3>Open Commitments</h3>
        ${openIntents
          .map(
            (i: any) => `
          <div class="intent-item action">
            <div class="intent-what">${escapeHtml(i.what || "")}</div>
            ${i.kind ? `<div class="intent-who">${escapeHtml(i.kind)}</div>` : ""}
            ${i.by_date ? `<div class="intent-date">by ${escapeHtml(i.by_date)}</div>` : ""}
          </div>`,
          )
          .join("")}
      </div>`);
  }

  if (recentMeetings.length > 0) {
    sections.push(`
      <div class="person-section"><h3>Recent Meetings</h3>
        ${recentMeetings
          .map(
            (m: any) => `
          <div class="person-meeting" data-path="${escapeAttr(m.path || "")}">
            <div class="person-meeting-date">${escapeHtml(m.date || "")}</div>
            <div class="person-meeting-title">${escapeHtml(m.title || "Untitled")}</div>
          </div>`,
          )
          .join("")}
      </div>`);
  }

  setInner($("person-body"), sections.join(""));

  // Click → detail for meetings
  $("person-body")
    .querySelectorAll(".person-meeting")
    .forEach((el) => {
      el.addEventListener("click", async () => {
        const path = (el as HTMLElement).dataset.path;
        if (!path) return;
        showView("loading");
        $("loading-text").textContent = "Loading meeting...";
        try {
          const result = await app.callServerTool({ name: "get_meeting", arguments: { path } });
          handleToolResult(result);
        } catch (e: any) {
          showError(e.message || "Failed to load meeting");
        }
      });
    });

  showView("person");
}

// ─── View: People Map ─────────────────────────────────────────────────────────

function renderPeopleMap(people: any[]) {
  const total = people.length;
  const losingCount = people.filter((p) => p.losing_touch).length;
  const withCommitments = people.filter((p) => p.open_commitments > 0).length;

  setInner(
    $("people-header"),
    `<h2>People</h2>
    <div class="stats-bar">
      <div class="stat"><span class="stat-value">${total}</span><span class="stat-label">Contacts</span></div>
      <div class="stat"><span class="stat-value">${withCommitments}</span><span class="stat-label">With Commitments</span></div>
      ${losingCount > 0 ? `<div class="stat stat-warn"><span class="stat-value">${losingCount}</span><span class="stat-label">Losing Touch</span></div>` : ""}
    </div>`,
  );

  const cards = people.map((p) => {
    const daysSince = Math.round(p.days_since || 0);
    const last = daysSince < 1 ? "today" : daysSince < 2 ? "yesterday" : `${daysSince}d ago`;
    const topics = (p.top_topics || []).slice(0, 3).join(", ");
    const statusClass = p.losing_touch ? "losing-touch" : p.open_commitments > 0 ? "has-commitments" : "clear";
    const statusText = p.losing_touch
      ? "⚠ losing touch"
      : p.open_commitments > 0
        ? `${p.open_commitments} commitment${p.open_commitments !== 1 ? "s" : ""}`
        : "✓ all clear";

    return `
    <div class="person-card ${statusClass}" data-name="${escapeAttr(p.name || "")}">
      <div class="person-card-name">${escapeHtml(p.name || "Unknown")}</div>
      <div class="person-card-meta">
        <span>${p.meeting_count || 0} meeting${(p.meeting_count || 0) !== 1 ? "s" : ""}</span>
        <span>last: ${escapeHtml(last)}</span>
      </div>
      ${topics ? `<div class="person-card-topics">${escapeHtml(topics)}</div>` : ""}
      <div class="person-card-status ${statusClass}">${statusText}</div>
      <div class="person-card-score">score: ${(p.score || 0).toFixed(1)}</div>
    </div>`;
  });

  setInner($("people-grid"), cards.join(""));

  // Click → person profile
  $("people-grid").querySelectorAll(".person-card").forEach((card) => {
    card.addEventListener("click", async () => {
      const name = (card as HTMLElement).dataset.name;
      if (!name) return;
      showView("loading");
      $("loading-text").textContent = `Loading ${name}...`;
      try {
        const result = await app.callServerTool({ name: "get_person_profile", arguments: { name } });
        handleToolResult(result);
      } catch (e: any) {
        showError(e.message || "Failed to load person");
      }
    });
  });

  // Empty state
  if (people.length === 0) {
    setInner($("people-grid"), `<div class="empty-state">No people found. Record some meetings, then click People again.</div>`);
  }

  // Back button → return to dashboard
  $("people-back-btn").onclick = () => {
    if (cachedDashboardData) {
      renderDashboard(cachedDashboardData);
    }
  };

  showView("people-map");
}

// ─── View: Report ─────────────────────────────────────────────────────────────

function renderReport(data: any) {
  const conflicts: any[] = data.decision_conflicts || [];
  const stale: any[] = data.stale_commitments || [];

  setInner($("report-header"), `<h1>Consistency Report</h1>`);

  if (conflicts.length === 0 && stale.length === 0) {
    setInner($("report-body"), `<div class="report-empty">No consistency issues found.</div>`);
    showView("report");
    return;
  }

  const sections: string[] = [];

  if (conflicts.length > 0) {
    sections.push(`
      <div class="report-section"><h3>Decision Conflicts</h3>
        ${conflicts
          .map(
            (c: any) => `
          <div class="intent-item conflict">
            <div class="intent-what">${escapeHtml(c.topic || "")}</div>
            ${c.latest ? `<div class="intent-source">Latest: &quot;${escapeHtml(c.latest.what || "")}&quot; (${escapeHtml(c.latest.title || "")})</div>` : ""}
          </div>`,
          )
          .join("")}
      </div>`);
  }

  if (stale.length > 0) {
    sections.push(`
      <div class="report-section"><h3>Stale Commitments</h3>
        ${stale
          .map(
            (s: any) => `
          <div class="intent-item stale">
            <div class="intent-what">${escapeHtml(s.entry?.what || "")}</div>
            ${s.entry?.who ? `<div class="intent-who">@${escapeHtml(s.entry.who)}</div>` : ""}
            <div class="intent-reasons">${escapeHtml(Array.isArray(s.reasons) ? s.reasons.join(", ") : `${s.age_days || "?"} days old`)}</div>
            ${s.latest_follow_up ? `<div class="intent-source">Latest: ${escapeHtml(s.latest_follow_up.title || "")}</div>` : ""}
          </div>`,
          )
          .join("")}
      </div>`);
  }

  setInner($("report-body"), sections.join(""));
  showView("report");
}

function renderContext(data: any) {
  const kind = data.kind || "context";
  const session = data.session || null;
  const events: any[] = Array.isArray(data.events) ? data.events : [];
  const links: any[] = Array.isArray(data.links) ? data.links : [];
  const topApps: any[] = Array.isArray(data.top_apps) ? data.top_apps : [];
  const topWindows: any[] = Array.isArray(data.top_windows) ? data.top_windows : [];
  const timeWindow = data.window || {};

  const sections: string[] = [];
  sections.push(
    `<div class="report-section">
      <div class="report-title">${escapeHtml(kind.replaceAll("_", " "))}</div>
      <div class="report-summary">${escapeHtml(timeWindow.start || "?")} → ${escapeHtml(timeWindow.end || "?")}</div>
    </div>`
  );

  if (session) {
    sections.push(
      `<div class="report-section">
        <div class="report-title">Session</div>
        <div class="report-summary">${escapeHtml(session.id || "")}</div>
      </div>`
    );
  }

  const renderTally = (title: string, entries: any[]) =>
    `<div class="report-section">
      <div class="report-title">${escapeHtml(title)}</div>
      <div class="intent-list">
        ${entries
          .map(
            (entry: any) => `
          <div class="intent-item">
            <div class="intent-what">${escapeHtml(entry.name || "unknown")}</div>
            <div class="intent-source">${escapeHtml(String(entry.count ?? 0))}</div>
          </div>`
          )
          .join("")}
      </div>
    </div>`;

  if (topApps.length > 0) sections.push(renderTally("Top Apps", topApps));
  if (topWindows.length > 0) sections.push(renderTally("Top Windows", topWindows));

  if (links.length > 0) {
    sections.push(
      `<div class="report-section">
        <div class="report-title">Linked Artifacts</div>
        <div class="intent-list">
          ${links
            .map(
              (link: any) => `
            <div class="intent-item">
              <div class="intent-what">${escapeHtml(link.kind || "")}</div>
              <div class="intent-source">${escapeHtml(link.target || "")}</div>
            </div>`
            )
            .join("")}
        </div>
      </div>`
    );
  }

  if (events.length > 0) {
    sections.push(
      `<div class="report-section">
        <div class="report-title">Events</div>
        <div class="intent-list">
          ${events
            .map(
              (event: any) => `
            <div class="intent-item">
              <div class="intent-what">${escapeHtml(event.app_name || event.bundle_id || "unknown")}${event.window_title ? ` :: ${escapeHtml(event.window_title)}` : ""}</div>
              <div class="intent-source">${escapeHtml(event.observed_at || "")}${event.url ? ` • ${escapeHtml(event.url)}` : ""}</div>
            </div>`
            )
            .join("")}
        </div>
      </div>`
    );
  }

  setInner($("report-body"), sections.join(""));
  showView("report");
}

// ─── Tool Result Router ──────────────────────────────────────────────────────

function handleToolResult(result: CallToolResult) {
  // Prefer structuredContent, fall back to _meta (host may not forward structuredContent)
  const sc = result.structuredContent as any;
  const meta = result._meta as any;
  const view: string | undefined = sc?.view || meta?.view;
  // Pick the source that actually has view-specific data, not just a truthy object
  const data = sc?.view ? sc : meta?.view ? meta : sc || meta || {};

  if (!view) {
    // No view hint — try to render text content as detail
    const text = result.content
      ?.map((c: any) => ("text" in c ? c.text : ""))
      .join("");
    if (text) {
      renderDetail({ content: text, path: "" });
    }
    return;
  }

  // For detail view, content may come from result.content instead of structuredContent
  if (view === "detail" && !data.content) {
    const textContent = result.content
      ?.map((c: any) => ("text" in c ? c.text : ""))
      .join("");
    if (textContent) data.content = textContent;
  }

  switch (view) {
    case "dashboard":
      renderDashboard(data);
      break;
    case "detail":
      renderDetail(data);
      break;
    case "person":
      renderPerson(data);
      break;
    case "relationship_map":
    case "commitments":
      renderPeopleMap(data.people || []);
      break;
    case "report":
      renderReport(data);
      break;
    case "context":
      renderContext(data);
      break;
    default:
      showError(`Unknown view: ${view}`);
  }
}

// Host → App: tool result from LLM-initiated tool call
app.ontoolresult = async (result: CallToolResult) => {
  handleToolResult(result);
};

// ─── Host Context (theme sync) ───────────────────────────────────────────────

function handleHostContext(ctx: McpUiHostContext) {
  if (ctx.theme) applyDocumentTheme(ctx.theme);
  if (ctx.styles?.variables) applyHostStyleVariables(ctx.styles.variables);
}

app.onhostcontextchanged = handleHostContext;

// ─── Connect ─────────────────────────────────────────────────────────────────

app.connect().then(() => {
  const ctx = app.getHostContext();
  if (ctx) handleHostContext(ctx);
});
