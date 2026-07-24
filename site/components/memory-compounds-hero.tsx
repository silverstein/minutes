"use client";

import type { MouseEvent } from "react";
import {
  GITHUB_CONTRIBUTORS,
  GITHUB_FORKS,
  GITHUB_STARS,
  NPM_MONTHLY_DOWNLOADS,
} from "@/lib/proof";
import { MINUTES_MCP_TOOL_COUNT } from "@/lib/release";

const primaryNav = [
  { label: "Product", href: "#product" },
  { label: "For agents", href: "/for-agents" },
  { label: "Compare", href: "/compare" },
  {
    label: "Resources",
    href: "/resources/best-meeting-tools-for-claude-code-and-codex",
  },
  { label: "Docs", href: "/docs" },
] as const;

function closeMobileMenu(event: MouseEvent<HTMLAnchorElement>) {
  const menu = event.currentTarget.closest("details");
  if (menu) menu.open = false;
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
              ? "border-b border-[color:var(--border)] py-3 font-mono text-[12px] uppercase tracking-[0.1em] text-[var(--text)]"
              : "hover:text-[var(--accent)]"
          }
        >
          {item.label}
        </a>
      ))}
      <a
        href="https://github.com/silverstein/minutes"
        onClick={mobile ? closeMobileMenu : undefined}
        className={
          mobile
            ? "border-b border-[color:var(--border)] py-3 font-mono text-[12px] uppercase tracking-[0.1em] text-[var(--text)]"
            : "hover:text-[var(--accent)]"
        }
      >
        GitHub
      </a>
    </>
  );
}

function MemorySequence() {
  return (
    <div
      className="memory-sequence"
      aria-label="How a voice memo becomes durable memory for an AI agent"
    >
      <article className="memory-step">
        <div className="memory-step-meta">
          <span>01</span>
          <span>09:41</span>
          <strong>Voice memo</strong>
        </div>
        <div className="memory-artifact memory-capture">
          <div className="memory-artifact-bar">
            <span>pricing-idea.m4a</span>
            <span>00:46</span>
          </div>
          <p>
            “Next three consultant signups get monthly. Annual stays the
            enterprise default.”
          </p>
          <span className="memory-capture-meta">
            Captured on Mac · transcribed locally · 4.2s
          </span>
        </div>
      </article>

      <article className="memory-step">
        <div className="memory-step-meta">
          <span>02</span>
          <span>09:42</span>
          <strong>Local file</strong>
        </div>
        <div className="memory-artifact memory-file">
          <div className="memory-artifact-bar">
            <span>~/meetings/2026-02-28-pricing-strategy.md</span>
            <span>0600</span>
          </div>
          <pre>
            <code>{`---
title: Pricing Strategy — Monthly Test
type: meeting
consent: verbal_all_parties
decisions:
  - text: Launch monthly billing for
      the next 3 consultant signups
    authority: high
---

## Transcript
[SPEAKER_0 1:02] Next three consultant
signups get monthly.`}</code>
          </pre>
        </div>
      </article>

      <article className="memory-step">
        <div className="memory-step-meta">
          <span>03</span>
          <span>Later</span>
          <strong>Recall</strong>
        </div>
        <div className="memory-artifact memory-recall">
          <div className="memory-recall-header">
            <span>Ambient context</span>
            <span>Minutes</span>
          </div>
          <p className="memory-prompt">
            What did we decide about consultant pricing?
          </p>
          <div className="memory-answer">
            <span className="memory-answer-mark">minutes</span>
            <p>
              Launch monthly billing for the next three consultant signups.
              Annual remains the enterprise default.
            </p>
            <span className="memory-source">
              Source: 2026-02-28-pricing-strategy.md · decision · high authority
            </span>
          </div>
        </div>
      </article>
    </div>
  );
}

export function MemoryCompoundsHero() {
  return (
    <>
      <header className="marketing-header">
        <nav
          className="marketing-nav"
          aria-label="Primary navigation"
        >
          <a
            href="/"
            className="font-mono text-[16px] font-semibold tracking-[-0.02em] text-[var(--text)]"
          >
            minutes
          </a>

          <div className="hidden items-center gap-7 text-[14px] text-[var(--text-secondary)] lg:flex">
            <NavigationLinks />
          </div>

          <a
            href="#install"
            className="hidden rounded-[4px] bg-[var(--accent)] px-5 py-2.5 font-mono text-[11px] font-semibold uppercase tracking-[0.1em] text-[#171411] hover:bg-[var(--accent-hover)] lg:inline-flex"
          >
            Download
          </a>

          <details className="marketing-mobile-menu lg:hidden">
            <summary>Menu</summary>
            <div className="marketing-mobile-links">
              <NavigationLinks mobile />
              <a
                href="#install"
                onClick={closeMobileMenu}
                className="mt-3 bg-[var(--accent)] px-4 py-3 text-center font-mono text-[12px] font-semibold uppercase tracking-[0.1em] text-[#171411]"
              >
                Download
              </a>
            </div>
          </details>
        </nav>
      </header>

      <section className="marketing-hero" aria-labelledby="home-title">
        <div className="marketing-hero-copy">
          <p className="marketing-eyebrow">
            Open source · Local first · MIT
          </p>
          <h1 id="home-title">
            Your AI remembers every conversation&nbsp;—
            <span>and no one can take it from you.</span>
          </h1>
          <p className="marketing-lede">
            Minutes turns meetings, voice memos, and dictation into durable
            local context—owned by you, readable by every AI you use. Nothing
            is uploaded.
          </p>

          <div className="marketing-actions">
            <a href="#install" className="marketing-primary-action">
              Download Minutes
            </a>
            <a href="#product" className="marketing-secondary-action">
              See Minutes in motion
            </a>
          </div>

          <p className="marketing-proof-line">
            {GITHUB_STARS} stars · {GITHUB_FORKS} forks ·{" "}
            {GITHUB_CONTRIBUTORS} contributors · {NPM_MONTHLY_DOWNLOADS} npm
            installs/mo
          </p>

          <div className="marketing-works-with">
            <span>One folder, every surface</span>
            <div>
              <strong>Desktop</strong>
              <strong>CLI</strong>
              <strong>Claude</strong>
              <strong>Codex</strong>
              <strong>Any MCP client</strong>
            </div>
          </div>
        </div>

        <div id="memory-flow" className="marketing-hero-product">
          <p className="marketing-product-kicker">
            One conversation. One durable file. Reliable recall.
          </p>
          <MemorySequence />
        </div>
      </section>

      <section className="marketing-proof-band" aria-label="Minutes proof points">
        <div className="marketing-proof-band-inner">
          <div>
            <span>Your folder of truth</span>
            <strong>Plain Markdown</strong>
            <p>Readable without Minutes. Ten years from now, grep still works.</p>
          </div>
          <div>
            <span>Network</span>
            <strong>0 uploads</strong>
            <p>Capture, transcription, and storage stay on your machine.</p>
          </div>
          <div>
            <span>Command surfaces</span>
            <strong>CLI + {MINUTES_MCP_TOOL_COUNT} MCP tools</strong>
            <p>Run Minutes in your shell or through Claude, Codex, Gemini, and any MCP client.</p>
          </div>
          <a href="/compare">
            <span>Choose by fit</span>
            <strong>Compare Minutes</strong>
            <p>See the differences in architecture, ownership, workflow, and tradeoffs.</p>
          </a>
        </div>
      </section>
    </>
  );
}
