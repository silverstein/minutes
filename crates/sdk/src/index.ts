// minutes-sdk — conversation memory for AI agents
//
// The "Mem0 for human conversations." Query meeting transcripts,
// decisions, action items, and people from any AI agent or app.
//
// Usage:
//   import { listMeetings, searchMeetings, defaultDir } from 'minutes-sdk';
//
//   const meetings = await listMeetings(defaultDir());
//   const results = await searchMeetings(defaultDir(), 'pricing');

export {
  // Types
  type ActionItem,
  type Decision,
  type Intent,
  type Frontmatter,
  type MeetingFile,
  type SpeakerAttribution,
  type SpeakerConfirmation,
  type AttributionSource,
  type CaptureSource,
  type CaptureWarning,
  type DiagnosticConfidence,
  type DiarizationPath,
  type FailureKind,
  type RecordingHealth,

  // Config
  defaultDir,

  // Parsing
  splitFrontmatter,
  parseFrontmatter,
  parseAttributionSource,

  // Query API
  listMeetings,
  searchMeetings,
  getMeeting,
  getMeetingWithOverlays,
  applySpeakerOverlays,
  humanizeTranscript,
  findOpenActions,
  findDecisions,
  getPersonProfile,
  listVoiceMemos,
} from "./reader.js";
