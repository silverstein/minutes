# Minutes homepage redesign — design QA

## Comparison target

- Source visual truth: `/Users/silverbook/.codex/generated_images/019f9180-f6dd-7212-ad6a-3bc4fd455f01/call_kT15gu1ZlKQeZQWUjz7eSy8t.png`
- Grounding reference: `/Users/silverbook/.codex/visualizations/2026/07/24/019f9180-f6dd-7212-ad6a-3bc4fd455f01/minutes-site-audit/07-installed-minutes-app-grounding.png`
- Final implementation screenshot: `/Users/silverbook/.codex/visualizations/2026/07/24/019f9180-f6dd-7212-ad6a-3bc4fd455f01/minutes-site-audit/19-redesign-final-desktop-v2.png`
- Final mobile screenshot: `/Users/silverbook/.codex/visualizations/2026/07/24/019f9180-f6dd-7212-ad6a-3bc4fd455f01/minutes-site-audit/18-redesign-final-mobile.png`
- CLI/copy follow-up screenshot: `/Users/silverbook/.codex/visualizations/2026/07/24/019f9180-f6dd-7212-ad6a-3bc4fd455f01/minutes-site-audit/22-redesign-cli-copy-final.png`
- Full-view comparison: `/Users/silverbook/.codex/visualizations/2026/07/24/019f9180-f6dd-7212-ad6a-3bc4fd455f01/minutes-site-audit/20-design-qa-final-side-by-side.png`
- Focused artifact comparison: `/Users/silverbook/.codex/visualizations/2026/07/24/019f9180-f6dd-7212-ad6a-3bc4fd455f01/minutes-site-audit/21-design-qa-artifact-focus.png`
- State: light theme, homepage at scroll position 0, mobile menu closed.

## Viewport and normalization

- Source pixels: 1672 × 941.
- Implementation browser CSS viewport: 1680 × 945 at device pixel ratio 1.
- Implementation screenshot pixels: 1665 × 937; the in-app browser capture excludes its scrollbar region.
- Full-view comparison normalized both images to 1200 × 675 and placed them side by side.
- Focused comparison used the product-evidence region from each original and normalized both crops to 900 × 760.
- Mobile verification used a 390 × 844 CSS viewport at device pixel ratio 1.

## Findings

No actionable P0, P1, or P2 findings remain.

### Fonts and typography

- The implementation uses the existing production Instrument Serif, Geist, and Geist Mono families, matching both the selected concept and the shipped Minutes design system.
- The canonical homepage promise replaces the concept's exploratory headline intentionally. It preserves the selected editorial hierarchy while keeping the stronger existing product and search language.
- The final headline has no orphaned em dash at desktop or mobile widths.

### Spacing and layout rhythm

- The asymmetric hero, left editorial copy field, right chronological evidence rail, compact navigation, and dark proof transition all match the selected composition.
- The proof band is visibly entering the 1680 × 945 fold, preserving the concept's forward momentum.
- The final desktop and mobile views have zero horizontal overflow.
- The local file artifact is fully visible; no product copy is clipped.

### Colors and visual tokens

- The implementation reuses the shipped cream, near-black, coral, elevated-paper, border, and shadow tokens.
- Secondary light-theme text was strengthened from `#8c8880` to `#6f6a62`; dark-theme secondary text was strengthened to `#aaa49a`.
- No gradients, glow, glass effects, or additional brand colors were introduced.

### Image quality and asset fidelity

- The selected concept does not require a photographic or illustrated raster asset.
- The generated concept's waveform and vendor marks were deliberately not recreated with CSS, inline SVG, or placeholder iconography.
- Product evidence is rendered as semantic interface content and is grounded in the shipped Minutes Markdown/frontmatter contract and the installed app's Ambient Context / Recall visual language.

### Copy and product truth

- The generated concept's imagined multi-file flow was corrected to the shipped contract: one Markdown artifact under `~/meetings/`, containing structured frontmatter and the transcript.
- The example uses real schema fields (`type`, `consent`, `decisions`, `authority`) and a sourced recall response.
- CLI now appears beside the desktop and MCP surfaces in the hero and proof band.
- The animated stats use the generated MCP tool count and no longer make a binary-size claim.
- Comparison links and editorial pages use direct, descriptive wording instead of self-certifying “honest” language.
- Comparison content remains substantial on the homepage, and the primary nav plus contextual comparison links strengthen crawl paths to `/compare` and key competitor pages.

### States, interactions, and responsiveness

- Desktop Product, For agents, Compare, Resources, Docs, GitHub, and Download links are present as ordinary server-rendered anchors.
- Mobile exposes the same primary links in a semantic `details` menu.
- Selecting a mobile menu link closes the menu before scrolling; the earlier overlay obstruction is fixed.
- Both hero actions work. `#memory-flow` lands below the sticky header, and `#install` lands with Mac and Windows choices visible.
- The mobile primary CTA is visible within the initial 390 × 844 viewport.
- Browser console warnings/errors: none.
- The revised `CLI + 36 MCP tools` proof point fits its cell without horizontal overflow.

### Accessibility

- Exactly one `h1` and one `main` landmark are present.
- Focus-visible outlines were added for links, buttons, and the mobile menu summary.
- Navigation content is present on desktop and mobile; no important crawl or user path is removed on the mobile surface.
- Reduced-motion behavior remains respected by the existing transcript animation rules.

## Comparison history

### Iteration 1

- Evidence: `08-redesign-desktop-v1.png`, `09-redesign-mobile-v1.png`, and `10-redesign-mobile-menu-v1.png`.
- [P2] The desktop headline orphaned its em dash and the proof band did not enter the fold.
- [P1] The mobile menu remained open after selecting Download and obscured the install choices.
- [P2] The local Markdown artifact clipped its final transcript lines.

### Fixes

- Kept “conversation —” together, widened the editorial column, tightened the desktop hero, and compressed the artifact rhythm.
- Added a small client-side close behavior to the semantic mobile `details` menu.
- Shortened the example to truthful, compact schema content and allowed the complete file excerpt to size naturally.

### Post-fix evidence

- `11-redesign-mobile-install-v2.png`: menu closed, `#install` top at 95.8 px, sticky header bottom at 59 px, Mac choice visible.
- `19-redesign-final-desktop-v2.png`: complete local file artifact, proof band visible by 116.6 px, no horizontal overflow.
- `20-design-qa-final-side-by-side.png`: final full-view source/implementation comparison.
- `21-design-qa-artifact-focus.png`: final focused product-evidence comparison.
- `22-redesign-cli-copy-final.png`: follow-up copy pass with Desktop, CLI, and MCP visible as distinct product surfaces.

## Follow-up polish

- P3: The coded product rail is deliberately calmer and more product-literal than the concept's four-stage illustration. A later motion pass could reveal the three shipped stages sequentially, using only opacity and position, without inventing new product chrome.

final result: passed
