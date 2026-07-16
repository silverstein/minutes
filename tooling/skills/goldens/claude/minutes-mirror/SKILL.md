---
name: minutes-mirror
description: Self-coaching analysis of your own behavior across meetings — talk-time ratio, filler words, hedging language, monologue length, energy patterns, and (when meetings are tagged via /minutes-tag) what your behavior in winning meetings looks like vs losing ones. Use this whenever the user says "how did I do", "review my last meeting", "mirror", "self-review", "show my patterns", "coach me", "where am I weak", "talk time", "am I improving", "what do I do in meetings I win", "feedback on me", or asks for any kind of personal feedback on their own meeting behavior. This is the rare skill that gives the user a mirror to their own habits — surface it whenever they show curiosity about their own performance, even if they don't use the word "mirror".
user_invocable: true
---

# /minutes-mirror

Self-coaching analysis based on your own meeting transcripts. Two modes:

- **Single-meeting mode** — review a specific meeting and surface what you did, what was unusual for you, and one concrete thing to try next time.
- **Pattern mode** — surface trends across the last 30 days, including (if meetings are tagged) what behaviors correlate with winning vs losing.

The point is not to roast you. The point is to give you a kind, evidence-based mirror to behaviors that are usually invisible to you because you're inside them.

## How it works

### Phase 0: Identify "you"

Mirror needs to know which speaker label in the transcript is the user. Real transcripts use one of two formats:

- **Enrolled users**: `[Mat 0:00] Hey there.` — first-name labels from voice enrollment
- **Non-enrolled users**: `[SPEAKER_0 0:00] Hey there.` — generic labels from diarization

Either way, mirror needs to know which label maps to the user. Check sources in order:

**1. Enrolled voice profile:**
```bash
minutes voices --json 2>/dev/null
```

Returns a JSON array of enrolled profiles. The user's profile is the one with `source: "self-enrollment"` (or the first one if there's only one). Use the `name` field as the speaker label to look for in transcripts. Example response:

```json
[{"person_slug": "mat", "name": "Mat", "source": "self-enrollment", ...}]
```

→ Speaker label is `Mat`.

**2. Cached self name(s):**
```bash
cat ~/.minutes/config/self.txt 2>/dev/null
```

The cache may contain **multiple labels**, one per line (e.g., `Mat`, `Mat S.`, `MAT_SILVERSTEIN`) — match any of them. People often appear under multiple labels across transcripts.

**3. Ask once and cache:**
If neither source returns a name, ask via AskUserQuestion: "Which speaker label in your transcripts is you? You can give multiple if you appear under different names (e.g., 'Mat, Mat S., MAT_SILVERSTEIN')."

Cache the answer (comma-separated input → one label per line):
```bash
mkdir -p ~/.minutes/config
printf '%s\n' <label1> <label2> ... > ~/.minutes/config/self.txt
```

This is a one-time setup cost. Don't ask again on future runs. If the user later mentions they have a new label, they can re-edit the file or re-run with `mirror reset-self`.

### Phase 1: Pick a mode

**Single-meeting mode** triggers on: "review my last meeting", "how did I do", "mirror that call", "feedback on the Sarah call".

**Pattern mode** triggers on: "show my patterns", "trends", "across all meetings", "coach me", "what do my winning meetings look like".

If ambiguous, default to **single-meeting mode on the most recent meeting** — it's fast, useful, and obviously what most people mean.

### Phase 2a: Single-meeting analysis

Find the target meeting (filter to meetings, not voice memos — talk-time analysis on a solo memo is meaningless):
```bash
minutes list --content-type meeting --limit 5
```
If the user named a specific meeting, use `minutes get <filename-without-.md>`. Otherwise pick the most recent.

**Compute the metrics with the bundled helper script**, not by counting in-context. LLMs are bad at exact token counting; the script does it deterministically with regex and basic string ops.

```bash
python3 "${CLAUDE_PLUGIN_ROOT}/skills/minutes-mirror/scripts/mirror_metrics.py" \
  "<path-to-meeting-file>" \
  --self "$(cat ~/.minutes/config/self.txt 2>/dev/null | paste -sd, -)"
```

The `--self` flag takes a comma-separated list of speaker labels (e.g., `Mat,Mat S.,SPEAKER_3`). Use the labels you cached in Phase 0.

The script outputs JSON to stdout with these fields:

| Field | Meaning |
|---|---|
| `total_words`, `self_words`, `other_words` | Word counts (split-on-whitespace) |
| `talk_ratio` | `self_words / total_words` as a 0–1 float |
| `self_turn_count`, `other_turn_count` | Number of speaker turns |
| `speakers` | All distinct speaker labels seen in the transcript |
| `filler_count`, `filler_per_100_words` | Filler-word hits in self speech (`um`, `uh`, `like`, `you know`, `basically`, `literally`, `kinda`, `right?`) |
| `hedging_count`, `hedging_per_100_words` | Hedging hits in self speech (`maybe`, `kind of`, `sort of`, `i think`, `i guess`, `possibly`, `somewhat`, `a little`, `perhaps`, `sorry to`). The word `just` is intentionally excluded — too many false positives. |
| `question_count`, `questions_per_5min` | Self questions (`?` count) |
| `duration_minutes` | From last timestamp if present, else word-count estimate at 150 wpm |
| `longest_monologue` | Longest uninterrupted self stretch: word count, seconds estimate, first 8 words, start time |
| `longest_listen` | Same shape, but for the longest stretch where you didn't speak |

The script exits non-zero on errors (file missing, no diarized turns, no self labels matched). On exit code 3 ("no turns matched any self label"), it tells you which speaker labels it found in the transcript — re-run with one of those, or update `~/.minutes/config/self.txt`.

**Compute your baseline by running the script on the last ~10 meetings.** Use Glob to enumerate them:

```
Glob: <output_dir>/*.md   (where <output_dir> comes from `minutes paths`)
```

Take the 10 most recent (filename starts with `YYYY-MM-DD-`), run `mirror_metrics.py` on each, average the metrics. If you have fewer than 5 prior meetings to average, say so explicitly — "Baseline computed from only N meetings, treat with caution" — instead of pretending the comparison is meaningful.

Once you have current-meeting metrics + baseline, flag anything >25% off baseline as worth noting.

**Output format:**

```markdown
## Mirror: <meeting title> · <date>

**Talk time**: You spoke <X>% of the time. (Your 30-day average: <Y>%.) <flag if abnormal>
**Longest monologue**: ~<N> seconds on "<topic>". <one-line judgment: was it earned (you were asked to explain something complex) or was it dominance?>
**Longest you listened**: ~<N> seconds during "<topic>". <one-line: what did they reveal?>
**Filler words**: <N> per 100 words. (Average: <Y>.)
**Hedging**: <N> per 100 words. (Average: <Y>.) <flag specific moments if you hedged on price, scope, or commitment>
**Questions asked**: <N>. <one-line: was this discovery, close, or update?>

### What stood out
<2–3 specific moments worth re-reading. Quote a short line from the transcript and say why it matters. Be specific — "You hedged the moment Sarah pushed on price ('I mean, I think we could maybe…')" beats "you hedged sometimes".>

### One thing to try next time
<Exactly one. Concrete. Achievable in the next call. Not a personality change — a behavior change. Falsifiable so the next mirror can verify it.>
```

### Phase 2b: Pattern mode

Run across the last 30 days (or whatever window the user gives you).

**Find the meeting directory** with `minutes paths` (look for the `output_dir:` line). Then use the **Glob tool** (not `ls`) to enumerate meeting files — Glob is more reliable than parsing shell output:

```
Glob pattern: <output_dir>/*.md
```

Filter to the requested window. Each meeting filename starts with `YYYY-MM-DD-` so you can filter by filename prefix without reading the file. For more precise filtering, parse the `date:` field from each meeting's frontmatter.

Compute the same per-meeting metrics across every meeting in the window. Then look for patterns:

**Behavioral patterns** (always available):
- Trend in talk ratio over time (going up = dominating more, going down = listening more)
- Topics that correlate with high talk ratio (where do you steamroll?)
- Topics that correlate with high hedging (where do you lose authority?)
- Filler word rate by time-of-day (fatigue curve?)
- Day-of-week patterns (worse on Mondays?)
- Meeting length patterns (do your >45-min meetings degrade?)

**Outcome correlations** (only if meetings are tagged via `/minutes-tag`):

Standard outcome tags that mirror correlates: `won`, `lost`, `stalled`, `great`, `noise`. These mirror the set defined by `/minutes-tag` — if that skill ever adds new standard tags, update mirror to recognize them too. Custom (non-standard) tags are ignored for correlation analysis.

Quickly check whether any meetings are tagged before reading them all:

```bash
grep -l "^outcome:" "$(minutes paths | grep '^output_dir' | awk '{print $2}')"/*.md 2>/dev/null
```

If that returns nothing, skip the outcome-correlation section entirely. Otherwise read the `outcome:` field from each tagged meeting's frontmatter, group by tag (`won` / `lost` / `stalled` / `great` / `noise`), and compare metrics across groups:

- "In meetings you tagged **won**, your average talk ratio was 38%. In **lost** meetings, 67%."
- "In **stalled** meetings, your hedging rate was 2× your baseline."
- "Every meeting you tagged **great** had ≥12 questions from you in the first 10 minutes."

**Minimum data thresholds:**
- **Behavioral patterns** need ≥5 meetings in the window to be meaningful. Below that, single-meeting mode is more honest.
- **Outcome correlations** need ≥3 meetings per tag group. Below that, it's noise.
- If thresholds aren't met, surface what you can compute and tell the user explicitly: "Tag more meetings via `/minutes-tag` and I can show you what wins look like."

**Output format:**

```markdown
## Mirror: 30-day patterns

**You've been in <N> meetings.** Here's what I see:

### Talk patterns
<2–3 bullets, specific>

### Where you hedge
<2–3 bullets with specific topics>

### Energy & timing
<observations about time-of-day, fatigue, day-of-week>

### Win/loss correlation
<only if ≥3 tagged meetings per outcome — otherwise skip this section entirely>

### One thing to try this week
<Exactly one. Concrete. Falsifiable.>
```

### Phase 3: Closing ritual

End with two beats:

1. **Specific experiment** — Restate the "one thing to try" as a concrete test. "Try cutting your hedging in your next 3 meetings. I'll measure it when you ask me to mirror again."

2. **Tag nudge** (only if no meetings have an `outcome:` field yet) — "After your next meeting, run `/minutes-tag won|lost|stalled` so I can correlate behavior with outcomes over time. ~10 tagged meetings is when the patterns get sharp."

## Gotchas

- **Long-transcript accuracy degrades.** LLMs are bad at exact token counting. For transcripts >5000 words, your filler-word and hedging counts are estimates, not measurements. Either say so in the output ("≈14 fillers, sampled from 3 segments") or sample three 1500-word segments (start, middle, end) and extrapolate. Don't pretend you exactly counted 8327 words.
- **This is coaching, not roasting.** Be specific, evidence-based, and kind. Quote actual lines from the transcript before making any judgment about tone or behavior. Never make claims you can't point to evidence for. The user is looking at themselves here — be the coach you'd want.
- **Speaker identification can fail.** If transcripts use generic labels like SPEAKER_0/SPEAKER_1 and the user hasn't enrolled their voice, the analysis can't know which speaker is them. Ask once per machine, cache forever in `~/.minutes/config/self.txt`.
- **Don't fake metrics.** If a transcript has no speaker diarization (one big block, no speaker labels), say so and offer pattern mode across other meetings instead. Don't compute talk-time on a transcript without speakers — the number will be wrong and the user will lose trust in everything else.
- **Word-count duration estimates are rough.** ~150 wpm is the convention. Use timestamps when present in the transcript; fall back to word count when not. Always say "≈" or "~" so the user knows it's an estimate.
- **Avoid corporate language.** Don't say "your engagement scores" or "talk-time KPI". Talk like a coach who actually cares: "you spoke 58% of the time" not "talk-time metric: 0.58".
- **Pattern mode needs at least 5 meetings.** Below that, single-meeting mode is more honest. Don't surface "trends" from 2 data points.
- **Outcome correlations need at least 3 per group.** Below that, it's noise. Tell the user the threshold and how to reach it.
- **Don't pathologize high talk time.** Sometimes talking 70% is correct — it's a presentation, you're delivering bad news, you're explaining something complex to a non-expert. Compare to baseline and note context. Don't treat any number as automatically bad.
- **The "one thing" must be testable.** "Be more confident" is useless. "Cut hedging words from your next 3 close calls" is testable. The user will either do it or not, and the next mirror should be able to verify.
- **Never compare across users.** Mirror is a mirror to **this** user, not a benchmark vs anyone else. Don't say "the average sales rep talks 45%". Compare the user only to themselves.
- **Hedging matters most around price, scope, and commitment.** A general filler-word count is interesting; flagging that the user hedged the moment Sarah pushed on price is useful. Surface where the hedging happened, not just how much.

