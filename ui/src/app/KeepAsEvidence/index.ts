// "Keep as evidence", for any screen that shows something the engine can
// capture itself: a chart (evidence.capture_chart), the map
// (evidence.capture_topology), the last lines of a terminal session
// (evidence.capture_terminal) or a page of a session log
// (evidence.capture_log) — and for text a person holds (evidence.add_text),
// which is labelled as theirs. See KeepAsEvidence.tsx for how to use it.
export {
  KeepAsEvidenceDialog,
  useKeepAsEvidence,
  type ChartEvidenceRequest,
  type KeepAsEvidence,
  type KeepAsEvidenceDialogProps,
  type KeepRequest,
  type LogEvidenceRequest,
  type TerminalEvidenceRequest,
  type TextEvidenceRequest,
  type TopologyEvidenceRequest,
} from './KeepAsEvidence';
export { captureCall, describeRequest, evidenceLink, refusal, type Refusal } from './keep';
