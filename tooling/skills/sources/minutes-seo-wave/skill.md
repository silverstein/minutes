---
name: minutes-seo-wave
description: Plan, build, and review one evidence-backed SEO content wave for the Minutes site, including SERP research, page inventory, design-system compliance, internal linking, generated LLM text, and a shipped-wave retro. Use when the user asks for an SEO wave, a group of comparison or use-case pages, a docs or resource hub expansion, or a coordinated batch of search landing pages.
triggers:
  - create an seo wave
  - build an seo wave
  - plan an seo wave
  - ship an seo wave
  - seo content wave
  - comparison page wave
  - use case page wave
  - docs hub wave
user_invocable: true
metadata:
  display_name: Minutes SEO Wave
  short_description: Ship a researched, internally linked SEO page wave without duplicating or overstating claims.
  default_prompt: Build one evidence-backed SEO content wave for Minutes and return the page checklist plus shipped-wave retro.
  site_category: Artifacts
  site_example: /minutes-seo-wave comparison pages
  site_best_for: Coordinate a researched batch of search pages, site integrations, and validation.
  site_visible: false
assets:
  scripts: []
  templates: []
  references: []
output:
  claude:
    path: .claude/plugins/minutes/skills/minutes-seo-wave/SKILL.md
  codex:
    path: .agents/skills/minutes/minutes-seo-wave/SKILL.md
tests:
  golden: true
  lint_commands: true
---

# /minutes-seo-wave

Produce one coherent SEO content wave, not a pile of disconnected pages. Keep the method reusable across comparison pages, use-case or resource pages, and docs hubs.

Competitive research and keyword specifics are time-sensitive. Research them fresh for each wave. Internal competitive plans may exist in gitignored documentation; never invent, reconstruct, or embed those private specifics in this skill or in repository documentation.

## Inputs

Collect:

- the wave theme, such as comparison pages, use-case pages, a resource cluster, or a docs hub
- target keywords and search intents
- the intended audience and conversion action
- the requested number of pages or the time box
- existing pages and drafts that must not be duplicated
- any supplied first-party research, approved claims, and source requirements

If theme or keywords are missing, inspect the repository and ask only for the choice that materially changes the wave. Do not invent search volume, keyword difficulty, competitor capabilities, pricing, compliance status, or product claims.

## Steps

### 1. Inventory the existing site

Map the current routes before proposing slugs or outlines:

```bash
rg --files site/app | rg '/page\.tsx$'
rg --files site/public | rg '\.md$'
sed -n '1,240p' site/app/sitemap.ts
```

Inspect relevant hubs, shared page components, adjacent pages, metadata, structured data, and markdown twins. Record pages that already satisfy the target intent, pages that should be refreshed instead of duplicated, and genuine content gaps.

### 2. Perform SERP recon

Research each target query with current search results. Identify:

- dominant intent: comparison, informational, transactional, navigational, or mixed
- recurring page formats and questions
- weak, stale, or unsupported answers the wave can improve
- primary sources required to support factual claims
- a distinct, truthful angle Minutes can substantiate

For competitor, legal, compliance, security, pricing, or product-capability claims, use current primary or official sources and record the review date. Treat search snippets and third-party summaries as discovery aids, not final evidence. For claims about Minutes, verify the current repository implementation and documentation.

Do not copy competitor framing or manufacture a claim because it would make a stronger headline. Give alternatives credit where they are better. Add appropriate not-legal-advice or scope language to regulated-topic pages.

### 3. Outline the wave page by page

For every proposed page, specify:

- route and primary keyword
- search intent and direct answer
- unique promise and evidence plan
- title, description, and social metadata
- section outline, including honest limitations or "when the alternative wins" where relevant
- structured data only when the visible page content supports it
- internal links in and out
- conversion action
- required hub, sitemap, and markdown-twin updates

Use extractable structures when they serve the query: a direct answer near the top, clear headings, comparison tables, concise lists, and visible FAQs that match any FAQ schema. Avoid near-duplicate pages that merely swap a product name or keyword.

### 4. Implement within `DESIGN.md`

Read `DESIGN.md` and reuse established site components and nearby page patterns. Preserve the Minutes type system: Instrument Serif for display headings, Geist for body and UI, and Geist Mono for labels, evidence, and technical material.

Use the repository's semantic design tokens and established utilities, including `var(--bg)`, `var(--text)`, `var(--accent)`, `font-serif`, `font-sans`, and `font-mono`. Do not introduce raw color values, ad hoc font families, off-scale spacing, gradients, glows, decorative motion, or new arbitrary literals. The design-token CI gate treats new raw literals as failures.

Keep the page useful without JavaScript when practical, keyboard accessible, responsive, and consistent in light and dark modes. Metadata and structured data must describe what the rendered page actually says.

### 5. Run an evidence and honesty pass

Fact-check every time-sensitive or consequential statement against its recorded source. Then compare Minutes claims against code and first-party documentation. Look especially for absolute language such as "never," "always," "nothing leaves," "compliant," "offline," or "free forever" that may need a precise boundary.

Check dates, plan names, pricing cadence, beta status, platform availability, default versus opt-in behavior, storage and network paths, and legal or compliance qualifiers. Remove a claim when it cannot be verified.

### 6. Complete the internal-linking pass

Connect the wave as a cluster:

- link each page to its relevant hub, pillar, sibling pages, and product or quick-start action
- add the new pages to the appropriate compare, resource, writing, or docs hub
- add contextual links from existing authoritative pages when useful
- update `site/app/sitemap.ts`
- add or update the corresponding `site/public/**/*.md` page when the site pattern requires a markdown twin
- verify there are no orphan pages or circular trails with no useful destination

Use descriptive anchor text. Do not stuff exact-match keywords into every link.

### 7. Regenerate and validate generated surfaces

After page and inventory changes, regenerate the LLM-facing site text:

```bash
node scripts/generate_llms_txt.mjs
node scripts/generate_llms_txt.mjs --check
```

Run the relevant site tests, type checks, and repository gates for the touched files. At minimum, run the design-token check when site UI changed:

```bash
node scripts/check_design_tokens.mjs
```

Do not hand-edit generated `llms.txt` files. Fix their source or generator input and regenerate.

## Output format

Return a page checklist followed by the shipped-wave retro. Keep routes, keywords, evidence, integrations, and validation visible:

```markdown
## Wave: <theme>

### Page checklist

- [ ] `/route-one` | `<primary keyword>` | <intent>
  - Direct answer and distinct angle: <one line>
  - Evidence: <primary sources or first-party repository evidence>
  - Integrations: <hub, sibling, sitemap, markdown twin, CTA>
- [ ] `/route-two` | `<primary keyword>` | <intent>
  - Direct answer and distinct angle: <one line>
  - Evidence: <primary sources or first-party repository evidence>
  - Integrations: <hub, sibling, sitemap, markdown twin, CTA>

### Wave checklist

- [ ] Existing `site/app/` and `site/public/` inventory checked for overlap.
- [ ] SERP intent and primary evidence recorded with a review date.
- [ ] `DESIGN.md`, accessibility, metadata, and structured-data constraints met.
- [ ] Hub, sibling, pillar, conversion, sitemap, and markdown-twin links complete.
- [ ] LLM text regenerated and repository checks pass.

## Shipped-wave retro

- **Scope shipped:** <routes and page types>
- **Why this wave:** <shared intent and evidence-backed opportunity>
- **Evidence and honesty review:** <sources checked, claims corrected or dropped, review date>
- **Site integration:** <hubs, internal links, sitemap, markdown twins, generated LLM text>
- **Validation:** <commands and results>
- **Follow-up:** <measurement or unresolved work, without inventing rankings or traffic>
```

Mark items complete only when the repository contains the work and the validation ran. The retro should follow the #435 through #438 pattern: summarize the shipped routes, the research basis, the integration work, the adversarial or honesty review, and concrete corrections. Do not claim ranking, traffic, or conversion impact before measurement exists.

## Checklist

Before calling the wave complete, verify:

- [ ] The theme, keywords, audience, conversion action, and wave size are explicit.
- [ ] Existing `site/app/` routes, hubs, sitemap entries, and markdown twins were inventoried.
- [ ] Each page answers a distinct search intent and avoids thin keyword substitution.
- [ ] Current primary sources support competitor and high-stakes claims.
- [ ] Minutes claims match current code and first-party documentation.
- [ ] `DESIGN.md` tokens and fonts are reused with no new raw design literals.
- [ ] Metadata, structured data, accessibility, responsive behavior, and light and dark modes were checked.
- [ ] Internal links connect every page to a hub, relevant siblings, a pillar, and a useful conversion action.
- [ ] `site/app/sitemap.ts` and required `site/public/**/*.md` twins are updated.
- [ ] `node scripts/generate_llms_txt.mjs --check` passes after regeneration.
- [ ] Relevant site checks and `node scripts/check_design_tokens.mjs` pass.
- [ ] The shipped-wave retro records scope, evidence, integrations, corrections, validation, and follow-up without invented outcomes.

