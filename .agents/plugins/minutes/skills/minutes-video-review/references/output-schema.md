# Output Schema

The script writes these artifacts into one bundle directory:

- `analysis.md`
- `analysis.json`
- `transcript.md`
- `metadata.json`
- `frames/`

## `metadata.json`

Expected fields:

- `source`
- `source_kind`
- `generated_at`
- `bundle_dir`
- `video_path`
- `video_duration_seconds`
- `video_has_audio_stream`
- `media_title`
- `media_description`
- `media_uploader`
- `transcript_method`
- `transcript_quality`
- `transcript_artifact`
- `analysis_artifact`
- `frames_dir`
- `contact_sheet_artifact`
- `frame_step_seconds`
- `max_frames`
- `sampled_frames`
- `focus`

## `analysis.json`

Expected top-level fields:

- `source`
- `source_kind`
- `generated_at`
- `focus`
- `transcript_method`
- `transcript_quality`
- `media_title`
- `media_description`
- `transcript_artifact`
- `metadata_artifact`
- `frames_dir`
- `contact_sheet_artifact`
- `analysis`

Expected `analysis` fields:

- `overall_summary`
- `sender_intent`
- `primary_signal`
- `confidence`
- `content_type`
- `review_mode`
- `likely_product_areas`
- `problem_signals`
- `proposal_signals`
- `evidence`
- `recommended_next_actions`
- `clarifying_questions`

This file may be generated from:

- heuristic logic only
- optional provider-assisted analysis

Agents should treat it as a strong starting point, not infallible ground truth.
