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
  type RestrictedMeetingStub,
  type ExactMeetingResult,
  type SpeakerAttribution,
  type SpeakerConfirmation,
  type AttributionSource,
  type CaptureSource,
  type CaptureWarning,
  type DiagnosticConfidence,
  type DiarizationPath,
  type FailureKind,
  type RecordingHealth,
  type ReadOptions,
  type OpenActionOptions,
  type PersonProfileOptions,
  type VoiceMemoOptions,
  SDK_MEETING_RESULT_MAX,
  SDK_VOICE_MEMO_RESULT_MAX,
  SDK_OPEN_ACTION_RESULT_MAX,
  SDK_DECISION_RESULT_MAX,
  SDK_PERSON_PROFILE_MEETING_MAX,
  SDK_PERSON_PROFILE_OPEN_ACTION_MAX,
  SDK_PERSON_PROFILE_TOPIC_MAX,
  SDK_VOICE_MEMO_LOOKBACK_MAX_DAYS,

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
  getDecisionHistory,
  getPersonProfile,
  listVoiceMemos,
} from "./reader.js";

export * from "./retrieval.js";
export * from "./evidence.js";
export * from "./decision-history.js";
export * from "./identity.js";
export * from "./hybrid-ranking.js";
