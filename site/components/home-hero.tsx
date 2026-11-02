"use client";

import type { MouseEvent } from "react";
import {
  APPLE_SILICON_DOWNLOAD_PATH,
  WINDOWS_INSTALLER_DOWNLOAD_URL,
} from "@/lib/downloads";
import {
  GITHUB_CONTRIBUTORS,
  GITHUB_FORKS,
  GITHUB_STARS,
  NPM_MONTHLY_DOWNLOADS,
} from "@/lib/proof";
import { MINUTES_RELEASE_VERSION } from "@/lib/release";

const primaryNav = [
  { label: "How it works", href: "#product" },
  { label: "Dictation", href: "#dictation" },
  { label: "For agents", href: "/for-agents" },
  { label: "Compare", href: "/compare" },
  { label: "Docs", href: "/docs" },
] as const;

/** "1,572" -> "1.6k", the compact form a nav link has room for.
 *
 * Reads the same hand-refreshed constant as the proof line below, so the nav
 * never shows a different number from the rest of the page.
 */
function compactCount(value: string): string {
  const count = Number(value.replace(/,/g, ""));
  if (!Number.isFinite(count) || count < 1000) return value;
  return `${(count / 1000).toFixed(1).replace(/\.0$/, "")}k`;
}

const GITHUB_STARS_COMPACT = compactCount(GITHUB_STARS);

/** GitHub's mark (Primer Octicons `mark-github`, MIT): what readers scan for
 * next to a star count. Inherits text color, so it follows the theme and hover.
 */
function GitHubMark() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true" className="h-4 w-4 shrink-0" fill="currentColor">
      <path d="M8 0c4.42 0 8 3.58 8 8a8.013 8.013 0 0 1-5.45 7.59c-.4.08-.55-.17-.55-.38 0-.27.01-1.13.01-2.2 0-.75-.25-1.23-.54-1.48 1.78-.2 3.65-.88 3.65-3.95 0-.88-.31-1.59-.82-2.15.08-.2.36-1.02-.08-2.12 0 0-.67-.22-2.2.82-.64-.18-1.32-.27-2-.27-.68 0-1.36.09-2 .27-1.53-1.03-2.2-.82-2.2-.82-.44 1.1-.16 1.92-.08 2.12-.51.56-.82 1.28-.82 2.15 0 3.06 1.86 3.75 3.64 3.95-.23.2-.44.55-.51 1.07-.46.21-1.61.55-2.33-.66-.15-.24-.6-.83-1.23-.82-.67.01-.27.38.01.53.34.19.73.9.82 1.13.16.45.68 1.31 2.69.94 0 .67.01 1.3.01 1.49 0 .21-.15.45-.55.38A7.995 7.995 0 0 1 0 8c0-4.42 3.58-8 8-8Z" />
    </svg>
  );
}

function closeMobileMenu(event: MouseEvent<HTMLAnchorElement>) {
  const menu = event.currentTarget.closest("details");
  if (menu) menu.open = false;
}

function GitHubStarsLink({ mobile = false }: { mobile?: boolean }) {
  return (
    <a
      href="https://github.com/silverstein/minutes"
      onClick={mobile ? closeMobileMenu : undefined}
      aria-label={`GitHub, ${GITHUB_STARS} stars`}
      className={
        mobile
          ? "flex items-center gap-2 border-b border-[color:var(--border)] py-3 text-[15px] text-[var(--text)]"
          : "inline-flex items-center gap-2 text-[14px] text-[var(--text-secondary)] hover:text-[var(--text)]"
      }
    >
      <GitHubMark />
      {mobile ? <span>GitHub</span> : null}
      <span>{GITHUB_STARS_COMPACT}</span>
    </a>
  );
}

function NavigationLinks({ mobile = false }: { mobile?: boolean }) {
  return (
    <>
      {primaryNav.map((item) => (
        <a
          key={item.label}
          href={item.href}
          onClick={mobile ? closeMobileMenu : undefined}
          className={
            mobile
              ? "border-b border-[color:var(--border)] py-3 text-[15px] text-[var(--text)]"
              : "hover:text-[var(--text)]"
          }
        >
          {item.label}
        </a>
      ))}
      {mobile ? <GitHubStarsLink mobile /> : null}
    </>
  );
}

export function SiteHeader() {
  return (
    <header className="site-header">
      <nav className="site-nav" aria-label="Primary navigation">
        <a href="/" className="flex items-center gap-2.5 text-[17px] font-semibold tracking-[-0.01em] text-[var(--text)]">
          <img src="/favicon.svg" alt="" width={24} height={24} className="rounded-[6px]" />
          Minutes
        </a>

        <div className="hidden items-center gap-7 text-[14px] text-[var(--text-secondary)] lg:flex">
          <NavigationLinks />
        </div>

        <div className="hidden items-center gap-5 lg:flex">
          <GitHubStarsLink />
          <a
            href="#install"
            className="inline-flex h-9 items-center rounded-full bg-[var(--text)] px-4 text-[14px] font-medium text-[var(--bg)] hover:opacity-90"
          >
            Download
          </a>
        </div>

        <details className="marketing-mobile-menu lg:hidden">
          <summary>Menu</summary>
          <div className="marketing-mobile-links">
            <NavigationLinks mobile />
            <a
              href="#install"
              onClick={closeMobileMenu}
              className="mt-4 rounded-full bg-[var(--text)] px-4 py-3 text-center text-[15px] font-medium text-[var(--bg)]"
            >
              Download
            </a>
          </div>
        </details>
      </nav>
    </header>
  );
}

const meetings = [
  { group: "March 25", items: [["Pricing follow-up", "10:00 AM · 34m"], ["Design review", "8:30 AM · 28m"]] },
  { group: "February 28", items: [["Pricing strategy: monthly test", "2:00 PM · 42m"], ["pricing-idea", "Voice memo · 0:46"]] },
] as const;

const waveHeights = [40, 70, 55, 90, 65, 100, 45, 80, 60, 95, 50, 75, 35, 85, 60, 70, 45, 90];

/** The Minutes desktop app, recreated in HTML: the meeting list with a
 * recording in progress, and Recall answering from a past meeting with its
 * source. Labels and layout follow the shipped app. */
function AppWindow() {
  return (
    <div className="app-window text-left" aria-label="The Minutes desktop app: a recording in progress and Recall answering a question with a source" role="img">
      <div className="flex items-center border-b border-[color:var(--border)] px-4 py-3">
        <div className="app-window-dots" aria-hidden="true">
          <span />
          <span />
          <span />
        </div>
      </div>
      <div className="grid md:grid-cols-[minmax(0,5fr)_minmax(0,7fr)]">
        <div className="flex flex-col border-b border-[color:var(--border)] p-5 md:border-b-0 md:border-r">
          <div className="flex items-center justify-between">
            <img src="/favicon.svg" alt="" width={26} height={26} className="rounded-[6px]" />
            <div className="flex gap-2 font-mono text-[11px] text-[var(--text-secondary)]">
              <span className="rounded-[6px] border border-[color:var(--border-mid)] px-2.5 py-1">Recall</span>
              <span className="rounded-[6px] border border-[color:var(--border-mid)] px-2.5 py-1">Documents</span>
            </div>
          </div>
          <div className="mt-4 rounded-[8px] border border-[color:var(--border-mid)] px-3 py-2 text-[13px] text-[var(--text-tertiary)]">
            Search recordings…
          </div>
          <div className="mt-4 flex-1 space-y-4">
            {meetings.map((block) => (
              <div key={block.group}>
                <p className="border-b border-[color:var(--border)] pb-1.5 font-mono text-[10px] uppercase tracking-[0.12em] text-[var(--text-tertiary)]">
                  {block.group}
                </p>
                {block.items.map(([title, meta]) => (
                  <div key={title} className="mt-2.5">
                    <p className="text-[13px] font-semibold text-[var(--text)]">{title}</p>
                    <p className="font-mono text-[11px] text-[var(--text-tertiary)]">{meta}</p>
                  </div>
                ))}
              </div>
            ))}
          </div>
          <div className="mt-6 rounded-[10px] border border-[color:var(--border-mid)] p-3">
            <div className="flex items-center justify-between gap-3">
              <div className="flex items-center gap-2 text-[13px] font-semibold text-[var(--capture)]">
                <span className="inline-block h-2 w-2 rounded-full bg-[var(--capture)]" aria-hidden="true" />
                Recording
              </div>
              <div className="app-wave" aria-hidden="true">
                {waveHeights.map((h, i) => (
                  <span key={i} style={{ height: `${h}%` }} />
                ))}
              </div>
              <span className="font-mono text-[12px] text-[var(--text)]">12:04</span>
            </div>
            <div className="mt-3 grid grid-cols-3 rounded-[8px] border border-[color:var(--border)] p-0.5 text-center font-mono text-[10px] uppercase tracking-[0.1em] text-[var(--text-tertiary)]">
              <span className="rounded-[6px] bg-[var(--bg-hover)] py-1.5 text-[var(--text)]">Meeting</span>
              <span className="py-1.5">Memo</span>
              <span className="py-1.5">Live</span>
            </div>
          </div>
        </div>

        <div className="flex flex-col p-5">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2 text-[14px] font-semibold text-[var(--text)]">
              <span className="inline-block h-2 w-2 rounded-full bg-[var(--text-tertiary)]" aria-hidden="true" />
              Recall
            </div>
            <span className="rounded-[6px] border border-[color:var(--border-mid)] px-2.5 py-1 font-mono text-[11px] text-[var(--text-secondary)]">
              Terminal
            </span>
          </div>
          <p className="mt-3 border-b border-[color:var(--border)] pb-3 font-mono text-[11px] text-[var(--text-tertiary)]">
            Using your Claude subscription in private Chat.
          </p>
          <div className="mt-4 self-end rounded-[10px] border border-[color:var(--border-mid)] bg-[var(--bg-elevated)] px-4 py-3 text-[13px] leading-6 text-[var(--text)] sm:max-w-[85%]">
            What is our current pricing decision, and what did it replace?
          </div>
          <div className="mt-5 space-y-3 text-[13px] leading-6 text-[var(--text-secondary)]">
            <p className="text-[17px] font-semibold tracking-[-0.01em] text-[var(--text)]">
              Current decision: annual-only billing
            </p>
            <p>
              Pricing went back to <strong className="font-semibold text-[var(--text)]">annual-only, effective immediately</strong>.
              It replaces the February 28 decision to test monthly billing with the next three consultant signups.
            </p>
            <p className="font-mono text-[11px] leading-5 text-[var(--text-tertiary)]">
              Source: 2026-03-25-pricing-reversal.md · SPEAKER_0 at 0:47 · decision
            </p>
          </div>
          <div className="mt-5 flex gap-2 md:mt-auto">
            <div className="flex-1 rounded-[8px] border border-[color:var(--border-mid)] px-3 py-2 text-[13px] text-[var(--text-tertiary)]">
              Ask about this meeting…
            </div>
            <span className="rounded-[8px] border border-[color:var(--border-mid)] px-3 py-2 font-mono text-[11px] font-semibold uppercase tracking-[0.08em] text-[var(--text)]">
              Send
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}

export function HomeHero() {
  return (
    <section className="mx-auto max-w-[1200px] px-6 pb-16 pt-14 sm:pt-20" aria-labelledby="home-title">
      <a
        href={`https://github.com/silverstein/minutes/releases/tag/v${MINUTES_RELEASE_VERSION}`}
        className="inline-flex items-center gap-2 rounded-full border border-[color:var(--border-mid)] px-3 py-1 text-[13px] text-[var(--text-secondary)] hover:text-[var(--text)]"
      >
        <span className="font-mono text-[12px] text-[var(--accent)]">v{MINUTES_RELEASE_VERSION}</span>
        <span className="hidden sm:inline">One-click Parakeet v3 and search by meaning on Mac</span>
        <span className="sm:hidden">What&apos;s new</span>
        <span aria-hidden="true">→</span>
      </a>
      <h1
        id="home-title"
        className="mt-6 max-w-[1100px] text-[40px] font-semibold leading-[1.04] tracking-[-0.04em] text-[var(--text)] sm:text-[60px] lg:text-[72px]"
      >
        Your conversations. Your memory.{" "}
        <span className="block text-[var(--text-tertiary)]">Ready for the AI you use.</span>
      </h1>
      <p className="mt-6 max-w-[640px] text-[17px] leading-7 text-[var(--text-secondary)] sm:text-[19px] sm:leading-8">
        Minutes records and transcribes your meetings, voice memos, and dictation{" "}
        <span className="text-[var(--text)]">on your own computer</span>, and keeps
        them as Markdown files that Claude, Codex, or any AI assistant can search.
        Free and open source.
      </p>

      <div className="mt-8 flex flex-wrap items-center gap-3">
        <a
          href={APPLE_SILICON_DOWNLOAD_PATH}
          data-download-target="mac"
          className="inline-flex h-12 items-center rounded-full bg-[var(--accent)] px-6 text-[16px] font-medium text-[var(--marketing-button-text)] hover:bg-[var(--accent-hover)]"
        >
          Download for Mac
        </a>
        <a
          href={WINDOWS_INSTALLER_DOWNLOAD_URL}
          data-download-target="windows"
          className="inline-flex h-12 items-center rounded-full border border-[color:var(--border-mid)] px-6 text-[16px] font-medium text-[var(--text)] hover:bg-[var(--bg-hover)]"
        >
          Windows
        </a>
        <a href="/for-agents#try" className="px-2 text-[15px] text-[var(--text-secondary)] underline decoration-[color:var(--border-mid)] underline-offset-4 hover:text-[var(--text)]">
          Try it with sample meetings
        </a>
      </div>
      <p className="mt-5 font-mono text-[12px] text-[var(--text-tertiary)]">
        {GITHUB_STARS} stars · {GITHUB_FORKS} forks · {GITHUB_CONTRIBUTORS} contributors ·{" "}
        {NPM_MONTHLY_DOWNLOADS} npm downloads a month · MIT
      </p>

      <div className="mt-12 sm:mt-14">
        <AppWindow />
      </div>
    </section>
  );
}
