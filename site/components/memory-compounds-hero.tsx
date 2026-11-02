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
  { label: "How it works", href: "#product" },
  { label: "For agents", href: "/for-agents" },
  { label: "Compare", href: "/compare" },
  {
    label: "Resources",
    href: "/resources",
  },
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

/** GitHub link with the star count. On desktop it sits beside Download rather
 * than among the page links, the way repo links usually sit in a nav's action
 * area; in the mobile menu it stays in the list, labelled.
 */
function GitHubStarsLink({ mobile = false }: { mobile?: boolean }) {
  return (
    <a
      href="https://github.com/silverstein/minutes"
      onClick={mobile ? closeMobileMenu : undefined}
      aria-label={`GitHub, ${GITHUB_STARS} stars`}
      className={
        mobile
          ? "flex items-center gap-2 border-b border-[color:var(--border)] py-3 font-mono text-[12px] uppercase tracking-[0.1em] text-[var(--text)]"
          : "inline-flex items-center gap-2 text-[14px] text-[var(--text-secondary)] hover:text-[var(--accent)]"
      }
    >
      <GitHubMark />
      {mobile ? <span>GitHub</span> : null}
      <span>{GITHUB_STARS_COMPACT}</span>
    </a>
  );
}

// Brand marks from Simple Icons (CC0, v16.36.0), drawn in the text color so
// they read as labels rather than decoration. Codex has no mark: OpenAI's logo
// is not in Simple Icons, and it is not redrawn here.
const surfaces = [
  { name: "Desktop" },
  { name: "CLI" },
  { name: "Claude", logo: "m4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.255h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.9021-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.1275.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.1275v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z" },
  { name: "Codex" },
  { name: "Gemini CLI", logo: "M11.04 19.32Q12 21.51 12 24q0-2.49.93-4.68.96-2.19 2.58-3.81t3.81-2.55Q21.51 12 24 12q-2.49 0-4.68-.93a12.3 12.3 0 0 1-3.81-2.58 12.3 12.3 0 0 1-2.58-3.81Q12 2.49 12 0q0 2.49-.96 4.68-.93 2.19-2.55 3.81a12.3 12.3 0 0 1-3.81 2.58Q2.49 12 0 12q2.49 0 4.68.96 2.19.93 3.81 2.55t2.55 3.81" },
  { name: "OpenCode", logo: "M22 24H2V0h20zM17 4.8H7v14.4h10z" },
  { name: "Any MCP client" },
] as const;

function BrandMark({ path }: { path: string }) {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" fill="currentColor">
      <path d={path} />
    </svg>
  );
}

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
      {mobile ? <GitHubStarsLink mobile /> : null}
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
title: Pricing Strategy; Monthly Test
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
            <span>Any MCP client</span>
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

          <div className="hidden items-center gap-6 lg:flex">
            <GitHubStarsLink />
            <a
              href="#install"
              className="inline-flex rounded-[4px] bg-[var(--accent)] px-5 py-2.5 font-mono text-[11px] font-semibold uppercase tracking-[0.1em] text-[var(--marketing-button-text)] hover:bg-[var(--accent-hover)]"
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
                className="mt-3 bg-[var(--accent)] px-4 py-3 text-center font-mono text-[12px] font-semibold uppercase tracking-[0.1em] text-[var(--marketing-button-text)]"
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
            Your conversations. Your memory.
            <span>Ready for the AI you use.</span>
          </h1>
          <p className="marketing-lede">
            Minutes is a free, open-source conversation memory app. Record and
            transcribe meetings, voice memos, and dictation locally. Let Claude,
            Codex, and other AI assistants search the history you choose to share.
          </p>

          <div className="marketing-actions">
            <a href="#install" className="marketing-primary-action">
              Download Minutes
            </a>
            <a href="/for-agents#try" className="marketing-secondary-action">
              Try it with your AI
            </a>
          </div>

          <p className="marketing-proof-line">
            {GITHUB_STARS} stars · {GITHUB_FORKS} forks ·{" "}
            {GITHUB_CONTRIBUTORS} contributors · {NPM_MONTHLY_DOWNLOADS} npm
            downloads/mo
          </p>

          <div className="marketing-works-with">
            <span>One folder, every surface</span>
            <div>
              {surfaces.map((surface) => (
                <strong key={surface.name}>
                  {"logo" in surface ? <BrandMark path={surface.logo} /> : null}
                  {surface.name}
                </strong>
              ))}
            </div>
          </div>
        </div>

        <div id="memory-flow" className="marketing-hero-product">
          <p className="marketing-product-kicker">
            One conversation. One durable file. Recall with a source.
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
            <span>Processing</span>
            <strong>Local by design</strong>
            <p>Capture and transcription stay local. Cloud AI receives only the context you authorize.</p>
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
