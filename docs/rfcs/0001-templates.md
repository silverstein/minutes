# RFC 0001: First-class templates for domain-shaped summaries

- **Status**: Draft
- **Authors**: @silverstein, @ed0c
- **Related**: #143
- **Created**: 2026-04-17

## Summary

Introduce **templates** as a first-class primitive in Minutes. A template is a markdown file with YAML frontmatter that extends Minutes' structured extraction for a specific domain (medical notes, standup summaries, sales discovery, legal intake, etc.). Templates are additive, not replacement: they preserve the baseline extraction contract (`KEY POINTS`, `DECISIONS`, `ACTION ITEMS`, `OPEN QUESTIONS`, `COMMITMENTS`, `PARTICIPANTS`) and layer custom extraction fields, agent context, compliance rules, and additional prompt instructions on top.

Templates ship bundled with Minutes, live in `~/.minutes/templates/` for user customization, and get contributed to a `templates/` directory in this repository as a community library.

## Motivation

Issue #143 surfaced a real limitation: the summarization prompt is hardcoded in `crates/core/src/summarize.rs`, which forces a recompile for any customization and offers no path to domain-specific output formats.

The straightforward reading of that ask is "add `--prompt` / `--prompt-file` flags." That approach breaks the pipeline: the current prompt isn't a casual default, it's the contract that produces the structured output parsed into YAML frontmatter. That frontmatter powers `minutes search-actions`, the MCP tools, the knowledge graph, hooks, skills, and the agent coaching loop. A replacement prompt would silently break all of that without an obvious error signal.

Templates solve the underlying problem (domain customization without recompile) while preserving the structured-extraction contract, and they unlock three things a prompt flag cannot:

1. **Vertical product surfaces** — per-template landing pages, SEO-indexable, each with a working example
2. **Community contribution** — medical, legal, therapy, and sales verticals need domain expertise Minutes will never have internally; templates let that expertise live in community-PRable markdown files
3. **Agent-aware domain semantics** — templates carry `agent_context` that informs Claude/Codex/Gemini/OpenCode when working with template-tagged meetings, without prompting users to re-explain context

## Non-goals

- Replace or bypass the baseline structured extraction
- Support arbitrary free-form prompts that produce unparseable output
- Build a separate package registry before a repo-based contribution workflow exists
- Duplicate skill functionality (templates define *schema*; skills define *interaction*)

## Prior art

- **Granola templates** — closed-source, opaque dropdown, single-vendor. The closest functional analog, and their moat. Minutes can do better by being markdown-native and community-driven.
- **Fathom AI Summary Templates** — similar dropdown UX
- **Fireflies SmartMatch** — keyword-triggered templates
- **Obsidian templates** — variable substitution, no LLM awareness
- **Rust RFCs / Python PEPs** — the RFC process itself; structured proposals with implementation phases

## Design

### Three-layer architecture this clarifies

- **Capture layer**: audio → transcript (Rust pipeline, whisper/parakeet, diarization). Unchanged.
- **Schema layer**: transcript → structured extraction, **template-aware**. This RFC defines this layer.
- **Interaction layer**: structured data → agent conversation via skills, MCP, hooks, graph. Templates can route to skills but don't replace them.

Templates and skills compose. A SOAP template guarantees the `subjective`/`objective`/`assessment`/`plan` shape in frontmatter; a `/minutes-soap-review` skill knows how to walk an agent through that shape conversationally. Template = schema; skill = interaction.

### Template anatomy

A template is a markdown file:

```yaml
---
name: Engineering Standup
slug: standup
version: 1.0.0
author: silverstein
license: MIT
description: Engineering standup summary with yesterday/today/blockers.
keywords: [standup, daily, engineering]
extends_base: true
triggers:
  calendar_keywords: [standup, daily, scrum]
  transcript_keywords: [yesterday, today, blocked]
extract:
  yesterday: "What was completed since the last standup"
  today: "Plans for today"
  blockers:
    technical: "Engineering blockers"
    cross_team: "Cross-team dependencies"
post_record_skill: minutes-standup-digest
agent_context: |
  This is an engineering standup. Keep responses engineering-specific.
additional_instructions: |
  Be concise. Blockers are the priority section.
language: en
---

# Engineering Standup Template

Human-readable documentation goes here: usage notes, examples, edge cases.
```

### Frontmatter fields

| Field | Type | Purpose |
|---|---|---|
| `name` | string | Human-readable name |
| `slug` | string | CLI identifier, URL-safe |
| `version` | semver | Template versioning for upgrades |
| `author` | string | Contributor credit |
| `license` | string | Per-template license (default MIT) |
| `description` | string | One-line summary (for listing + SEO) |
| `keywords` | [string] | Search + SEO |
| `extends` | slug (optional) | Inherit from another template (see Inheritance) |
| `extends_base` | bool | If true, baseline structured extraction still runs and custom fields layer on top. Default true. |
| `triggers.calendar_keywords` | [string] | Auto-select template from calendar event title |
| `triggers.transcript_keywords` | [string] | Post-hoc suggestion if no template was picked |
| `extract` | object | Custom extraction schema. Values can be strings (descriptions) or nested objects (sub-fields). Capped at 3 levels. |
| `post_record_skill` | string | `/minutes-*` skill invoked by post-record hook |
| `agent_context` | string | Injected into LLM prompts for agents working with this meeting later |
| `compliance` | object | Declarative rules (see Compliance) |
| `additional_instructions` | string | Appended to base system prompt (NEVER replaces) |
| `language` | string | Override `[summarization] language` for this template |

### Storage and resolution

Templates are resolved in this order (earlier wins):

1. Project: `.minutes/templates/` (repo-local, checked into git)
2. User: `~/.minutes/templates/`
3. Bundled: shipped with the binary (`crates/assets/templates/`)
4. Community: `templates/` in this repository, distributed with releases

Users can override bundled templates by putting a file with the same slug in `~/.minutes/templates/`.

### Inheritance (family pattern)

Templates can extend another template via `extends:`. Common use case: shared compliance + language + agent_context across a family of domain-specific templates.

```yaml
# medical-fr-base.md
---
name: Medical (French, base)
slug: medical-fr-base
language: fr
compliance:
  redact_phi: true
  require_local_processing: true
agent_context: |
  This is clinical data under French HDS and RGPD.
additional_instructions: |
  Utilisez la terminologie clinique française.
---
```

```yaml
# consultation-fr.md
---
name: Consultation Report (French)
slug: consultation-fr
extends: medical-fr-base
extract:
  motif_consultation: "Motif de la consultation"
  anamnese: "Anamnèse"
  examen_clinique: "Résultats de l'examen clinique"
  assessment:
    diagnostic_principal: "Diagnostic principal"
    diagnostics_differentiels: "Diagnostics différentiels"
  plan:
    traitement: "Traitement prescrit"
    suivi: "Suivi recommandé"
---
```

Inheritance rules:
- Child inherits: `compliance`, `agent_context`, `additional_instructions`, `language`
- Child overrides: `extract`, `triggers`, `post_record_skill`, `name`, `description`, `keywords`
- Child merges (with conflict warning): `compliance` individual fields, `additional_instructions` (concat)
- Inheritance is single-parent (no multiple inheritance); grandparents resolve transitively

### Extract fields

`extract:` accepts either a string (a description for a flat field) or a nested object (sub-fields, each with its own string or nested-object value). Nesting is capped at 3 levels.

```yaml
extract:
  subjective: "Patient-reported symptoms and history"          # flat
  objective: "Exam findings, vitals, labs"
  assessment:                                                   # nested
    diagnosis: "Primary clinical impression"
    differential: "Differential diagnoses"
  plan:
    treatment:                                                  # 3rd level
      medications: "Prescribed medications with dosing"
      procedures: "Procedures performed"
    followup: "Next steps and referrals"
```

The summarizer converts the `extract:` tree into a JSON schema, passes it to the LLM as structured-output guidance, and round-trips the result back into YAML frontmatter with the nested shape intact.

Reliability note: depth > 3 levels is unreliable with current open-weights models and will produce a validation error at template-load time.

### Triggers and auto-selection

If `--template` isn't explicitly provided on the command line, Minutes picks a template in this order:

1. Match `triggers.calendar_keywords` against the upcoming/current calendar event title (requires calendar integration enabled)
2. After transcription, match `triggers.transcript_keywords` against the transcript content
3. Fall back to the `meeting` template

Manual override: `minutes record --template <slug>` or `minutes process <file> --template <slug>`.

### Compliance

The `compliance` field encodes declarative rules checked by the pipeline:

| Field | Type | Behavior |
|---|---|---|
| `redact_phi` | bool | Post-extraction, redact likely PHI patterns (names, DOBs, phone numbers, MRNs) before persistence |
| `forbid_in_summary` | [string] | Enum of `[phone_number, full_ssn, full_dob, full_name, email, mrn]`; validator rejects summary if detected |
| `require_local_processing` | bool | If true, Minutes errors when a cloud summarization engine (Claude, OpenAI, Mistral) is configured |
| `retention_days` | int | Annotates frontmatter with `retention_until`; downstream tools can enforce deletion |
| `audit_log` | bool | Writes a timestamped entry to `~/.minutes/logs/audit.log` (template, action, file hash) |

Compliance rules compose through inheritance. Child can tighten (stricter) but a warning fires if child loosens a parent's rule.

### CLI surface

```bash
minutes template list                         # list installed templates
minutes template show <slug>                  # dump template contents
minutes template install <url|gh:user/repo>   # install from URL / gh repo / gist
minutes template search <query>               # search gallery + installed
minutes template create <name>                # scaffold new template with heredoc
minutes template validate <path>              # schema check + smoke test
minutes template upgrade                      # check for template updates

minutes record --template <slug>              # record with explicit template
minutes process <file> --template <slug>      # re-process existing recording with new template
```

### Agent integration

When an agent (Claude Desktop, Code, Cowork, Codex, Gemini, OpenCode) interacts with a template-tagged meeting via MCP:

- The meeting's frontmatter includes `template: <slug>` and the full extracted shape
- MCP tool responses include `agent_context` from the template, injected as guidance
- Skills declared via `post_record_skill` are invoked automatically on record completion

The interaction layer stays skill-driven; templates just enrich what skills have to work with.

## Worked examples

### `meeting` (bundled, baseline)

Minimal default, used when no other template matches.

```yaml
---
name: Meeting
slug: meeting
version: 1.0.0
description: Generic meeting summary (default).
extends_base: true
---
```

### `standup` (bundled)

```yaml
---
name: Engineering Standup
slug: standup
extends_base: true
triggers:
  calendar_keywords: [standup, daily, scrum]
extract:
  yesterday: "What was completed since last standup"
  today: "Plans for today"
  blockers:
    technical: "Engineering blockers"
    cross_team: "Cross-team dependencies"
post_record_skill: minutes-standup-digest
---
```

### `medical-fr-base` (community, co-authored with @ed0c)

See Inheritance section above.

### `consultation-fr` (community, extends `medical-fr-base`)

See Inheritance section above.

### `soap-fr` (community, extends `medical-fr-base`)

```yaml
---
name: SOAP (French)
slug: soap-fr
extends: medical-fr-base
description: Note SOAP en français pour consultations médicales
extract:
  subjective: "Symptômes rapportés par le patient"
  objective: "Examen clinique, signes vitaux, résultats de laboratoire"
  assessment:
    diagnostic: "Diagnostic principal"
    differentiel: "Diagnostics différentiels"
  plan:
    traitement: "Traitement"
    suivi: "Suivi et orientations"
post_record_skill: minutes-soap-review
---
```

## Implementation phases

### Phase 1 — shippable, resolves #143
- `crates/core/src/template.rs`: Template struct, loader, resolver (project > user > bundled)
- `additional_instructions` appended to `build_system_prompt` (base prompt preserved)
- `--template <slug>` flag in CLI
- Bundled templates: `meeting`, `standup`, `1-on-1`, `voice-memo`
- CLI: `minutes template list`, `show`, `validate`
- Tests: loader, resolver, prompt composition, schema validation

### Phase 2 — custom extract fields
- Summarizer reads `extract:` from active template, requests structured output from LLM
- Nested objects supported (max 3 levels)
- YAML frontmatter writer extends output with custom fields
- Reader crate (`crates/reader/`, `crates/sdk/src/reader.ts`) passes custom fields through
- MCP tools expose custom fields in responses
- Ship: `interview`, `sales-discovery`, `lecture`

### Phase 3 — compliance + post_record_skill
- Compliance rules enforced pre-persistence (redaction, validation)
- `post_record_skill` wired into post-record hook
- `agent_context` injected into MCP responses
- Ship regulated verticals: `soap`, `medical-fr-base`, `consultation-fr`, `soap-fr`, `therapy-intake`, `legal-consult`
- Audit log infrastructure

### Phase 4 — calendar routing, community gallery, inheritance
- Calendar keyword auto-selection
- `templates/` dir in repo accepting community PRs
- Template validation in CI (schema + smoke test against `example.md`)
- `minutes template install` for gh repos + gists
- `extends:` inheritance resolution
- Gallery landing page on useminutes.app

### Phase 5 — graph schema extensions
- Custom extract fields flow into `graph.db`
- Domain-aware MCP graph tools (query across medical, legal, sales templates)
- Cross-template queries ("all SOAP notes where diagnosis includes X")

## Open questions

These are the areas where community input most changes the shape. Comments welcome on any of them.

1. **Multi-template composition**: Some recordings straddle domains (a lunch conversation that's half personal + half clinical). Should Minutes support `--templates soap,voice-memo` that merges? My lean is no, too much complexity for marginal benefit, but open to a strong use case.

2. **Template signing**: Community templates declare `agent_context` that gets injected into LLM prompts, which is adjacent to prompt-injection territory. Should templates be signed, reviewed first-party, or sandboxed? My lean: first-party review via PR is enough for Phase 4, signing is a Phase 6+ concern.

3. **Template versioning and re-processing**: If a template bumps major version, do previously-processed meetings get re-processed? My lean: immutable by default; `minutes process <file> --template soap@2.0` is an explicit re-processing action. `version` in frontmatter records which template version was used.

4. **Engine compatibility**: Some templates may rely on structured-output features of specific LLMs (JSON mode in OpenAI/Claude). Should templates declare `supported_engines:` and error loudly on incompatible engines? My lean: yes, fail fast is better than silent degradation, especially for compliance-sensitive templates where partial extraction could be worse than none.

5. **Live transcript interaction**: Live mode streams utterances incrementally. Does the `extract:` schema progress during a live meeting, or only finalize at stop? My lean: finalize at stop for Phase 2 simplicity; streaming structured extraction is a later design.

6. **Compliance extensibility**: Should `compliance` be pluggable so that a Rust plugin could define new rules (e.g., `hipaa_bulk_export_check`)? My lean: Phase 5 or later; start with the fixed enum in `forbid_in_summary` and expand as needs surface.

7. **Inheritance depth**: Single-parent with transitive grandparents, or should we support multiple inheritance (mixins)? My lean: single-parent, explicit and auditable.

8. **Template search / discovery**: CLI `minutes template search` queries bundled + installed; should it also query the gh repo's `templates/` directory? My lean: yes, via a periodic index refresh, not live queries per search.

## Acknowledgments

- **@ed0c** (#143) surfaced the limitation and agreed to co-author the regulated-vertical reference implementation. The HDS + RGPD constraints shaped the compliance field directly.

## Next steps

- Collect feedback on this RFC for ~2 weeks
- @ed0c and @silverstein co-author first draft of `medical-fr-base`, `consultation-fr`, `soap-fr`
- Begin Phase 1 implementation on a separate branch
- RFC merges once feedback has converged
