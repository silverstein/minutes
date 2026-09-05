import type { Metadata } from "next";

/** Shared JSON-LD builders for structured data.
 *
 * Schema.org markup must describe content that is actually visible on the page.
 * Every field produced here is derived from data the page already renders, so
 * the markup cannot drift from the copy.
 *
 * That rule is why `faqPageSchema` takes the same array the page renders through
 * `<FaqSection>` rather than a separate literal. Until 2026-08-09 every resource
 * page hand-rolled FAQPage JSON-LD that appeared nowhere in its visible copy, a
 * structured-data violation Google can penalize. Pairing the builder with the
 * component makes the compliant thing the easy thing: you cannot emit the markup
 * without also rendering the content.
 *
 * Comparison pages still emit no FAQPage, because they have no FAQ section.
 */

const SITE_URL = "https://useminutes.app";
const REPO_URL = "https://github.com/silverstein/minutes";

/** Absolute URL for a site-relative path, as schema.org requires. */
function absolute(path: string): string {
  return path.startsWith("http") ? path : `${SITE_URL}${path}`;
}

/** Mat Silverstein as the named author, for E-E-A-T author attribution. */
function author() {
  return {
    "@type": "Person",
    name: "Mat Silverstein",
    url: REPO_URL.replace("/minutes", ""),
  };
}

/** The Minutes project as the publishing entity. */
export function organizationSchema() {
  return {
    "@context": "https://schema.org",
    "@type": "Organization",
    name: "Minutes",
    url: SITE_URL,
    logo: absolute("/favicon.svg"),
    description:
      "Open-source, privacy-first conversation memory. Records meetings and voice memos, transcribes and diarizes them on your own device, and writes searchable markdown you own.",
    founder: author(),
    sameAs: [REPO_URL],
  };
}

/** Minutes as a software product.
 *
 * Free and MIT licensed, which is the differentiator an AI agent needs to be
 * able to parse when it compares tools on a buyer's behalf.
 */
export function softwareApplicationSchema() {
  return {
    "@context": "https://schema.org",
    "@type": "SoftwareApplication",
    name: "Minutes",
    url: SITE_URL,
    applicationCategory: "BusinessApplication",
    applicationSubCategory: "Conversation memory and meeting transcription",
    operatingSystem: "macOS, Linux, Windows",
    description:
      "Free, open-source conversation memory for Claude, Codex, and other AI assistants. Capture, transcription, and Markdown storage run locally. Optional cloud AI receives the meeting context you authorize.",
    license: "https://opensource.org/licenses/MIT",
    isAccessibleForFree: true,
    offers: {
      "@type": "Offer",
      price: "0",
      priceCurrency: "USD",
    },
    author: author(),
    sameAs: [REPO_URL],
  };
}

/** A competitor referenced by a comparison page.
 *
 * Declaring both products as `about` entities is what lets an answer engine
 * resolve a "X vs Y" query to this page. Page labels sometimes carry a
 * disambiguating parenthetical ("Hyprnote (Anarlog)") that reads as part of the
 * product name to a matcher, so it is dropped from the entity name only.
 */
function competitorSchema(label: string) {
  return {
    "@type": "SoftwareApplication",
    name: label.replace(/\s*\([^)]*\)\s*$/, ""),
    applicationCategory: "BusinessApplication",
  };
}

/** One question and its answer, rendered visibly and emitted as markup. */
export type FaqItem = { question: string; answer: string };

/** Structured data for a page's FAQ.
 *
 * Pass the exact array the page renders through `<FaqSection>`. Google requires
 * FAQPage content to be visible to the user, so emitting this for questions that
 * only exist in JSON-LD is a violation, not a shortcut.
 */
export function faqPageSchema(items: ReadonlyArray<FaqItem>) {
  return {
    "@context": "https://schema.org",
    "@type": "FAQPage",
    mainEntity: items.map((item) => ({
      "@type": "Question",
      name: item.question,
      acceptedAnswer: { "@type": "Answer", text: item.answer },
    })),
  };
}

type ResourceSchemaInput = {
  /** The page's own `metadata` export, so title and description cannot drift. */
  metadata: Metadata;
  /** Site-relative canonical path, e.g. "/resources/remove-otter-ai-from-zoom". */
  path: string;
  /** ISO date the page was last fact-checked; becomes dateModified. */
  lastReviewed: string;
  /** The page's visible Sources list, emitted as citations. */
  sources?: ReadonlyArray<{ label: string; href: string }>;
};

/** Next's Metadata title is a union; every resource page uses a plain string. */
function titleText(title: Metadata["title"]): string {
  if (typeof title === "string") return title;
  if (title && typeof title === "object") {
    if ("absolute" in title && typeof title.absolute === "string") {
      return title.absolute;
    }
    if ("default" in title && typeof title.default === "string") {
      return title.default;
    }
  }
  return "";
}

/** Structured data for a `/resources/*` guide or answer page.
 *
 * These pages carry the two signals that drive AI citation, sourced claims and
 * a visible review date, but only in prose. This makes both machine-readable.
 * Title and description are read from the page's own `metadata` export rather
 * than restated, so the markup cannot disagree with the `<title>`.
 *
 * Emit alongside `faqPageSchema` where the page has a visible FAQ; the two are
 * separate top-level objects in one JSON-LD array.
 */
export function resourceArticleSchema({
  metadata,
  path,
  lastReviewed,
  sources = [],
}: ResourceSchemaInput) {
  return {
    "@context": "https://schema.org",
    "@type": "TechArticle",
    headline: titleText(metadata.title),
    description: metadata.description ?? "",
    url: absolute(path),
    mainEntityOfPage: { "@type": "WebPage", "@id": absolute(path) },
    dateModified: lastReviewed,
    inLanguage: "en",
    author: author(),
    publisher: organizationSchema(),
    citation: sources.map((source) => ({
      "@type": "CreativeWork",
      name: source.label,
      url: source.href,
    })),
  };
}

type ComparisonSchemaInput = {
  /** Competitor as titled on the page, e.g. "Granola AI". */
  competitorLabel: string;
  /** Site-relative canonical path, e.g. "/compare/granola-vs-minutes". */
  path: string;
  /** The page's hero summary, reused verbatim as the description. */
  description: string;
  /** ISO date the page was last fact-checked; becomes dateModified. */
  lastReviewed: string;
  /** The page's visible Sources list, emitted as citations. */
  sources: ReadonlyArray<{ label: string; href: string }>;
};

/** Structured data for a "Minutes vs X" comparison page.
 *
 * Comparison content is the most-cited format in AI answers, and the two
 * signals that drive citation are sourced claims and a visible review date.
 * Both already exist on these pages; this makes them machine-readable.
 */
export function comparisonArticleSchema({
  competitorLabel,
  path,
  description,
  lastReviewed,
  sources,
}: ComparisonSchemaInput) {
  return {
    "@context": "https://schema.org",
    "@type": "TechArticle",
    headline: `Minutes vs ${competitorLabel}`,
    description,
    url: absolute(path),
    mainEntityOfPage: { "@type": "WebPage", "@id": absolute(path) },
    dateModified: lastReviewed,
    inLanguage: "en",
    author: author(),
    publisher: organizationSchema(),
    about: [
      { "@type": "SoftwareApplication", name: "Minutes", url: SITE_URL },
      competitorSchema(competitorLabel),
    ],
    citation: sources.map((source) => ({
      "@type": "CreativeWork",
      name: source.label,
      url: source.href,
    })),
  };
}
