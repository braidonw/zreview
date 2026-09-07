import type {
  AnchoredDraftDto,
  DraftsDto,
  FileDetailDto,
  FindingDto,
  ReviewPanelDto,
  SessionFailureDto,
  SessionSnapshotDto,
  SidebarDto,
  SubmissionDto,
} from "../bindings";
import { clamp } from "../lib/clamp";

/** Shown under the composer when a span was refused rather than saved. */
const REJECTED_NOTICE = "This selection cannot hold a comment";

/** What the summary editor holds, with a count that moves only on a backend replacement. */
export type SummaryState = { body: string; loads: number };

/** The composer's frozen span and any notice it is showing, or absent when closed. */
export type ComposerState = { rows: [number, number]; notice: string | null } | null;

/**
 * The reviewer's own text and a finding's proposal, shown while accepting asks
 * whether to replace what is already there.
 */
export type FindingConflict = { id: number; existing: string; proposed: string } | null;

export type SessionState =
  | { status: "loading"; description: string; stage: string }
  | { status: "failed"; failure: SessionFailureDto }
  | {
      status: "ready";
      snapshot: SessionSnapshotDto;
      file: FileDetailDto;
      cursor: number;
      anchor: number;
      drafts: DraftsDto;
      composer: ComposerState;
      /** The review panel, absent for a snapshot that cannot be reviewed at all. */
      panel: ReviewPanelDto | null;
      /**
       * What a review command refused, shown inside the panel.
       *
       * A refused toggle or a cancel that could not be delivered is about the
       * review, not the sitting. Replacing the Session with a failure screen
       * would throw away the diff and whatever is unsent in the composer.
       */
      panelNotice: string | null;
      /** The confirmation accepting a finding onto an occupied line is asking. */
      findingConflict: FindingConflict;
      /**
       * How far a submission has got, held in memory only.
       *
       * Never persisted, so a fresh Session starts idle however the last one
       * ended.
       */
      submission: SubmissionDto;
      /**
       * What the summary editor holds, and how many times the backend has
       * replaced it.
       *
       * `body` moves with every keystroke, so this is never a stale copy of what
       * is on screen. `loads` moves only when the backend replaces the text,
       * which is a whole-change finding merged in or a landed review emptying
       * it, and is what tells the editor to take the new text. Comparing the
       * text itself cannot do that job, because React state trails the editor by
       * a keystroke and an equality check ends up loading an older document over
       * a newer one, eating what was just typed.
       */
      summary: SummaryState;
    };

export type ReadyState = Extract<SessionState, { status: "ready" }>;

export const initialState: SessionState = {
  status: "loading",
  description: "",
  stage: "Starting",
};

export type SessionAction =
  | { type: "describe"; description: string }
  | { type: "stage"; stage: string }
  | { type: "failed"; failure: SessionFailureDto }
  | {
      type: "ready";
      snapshot: SessionSnapshotDto;
      file: FileDetailDto;
      panel: ReviewPanelDto | null;
    }
  | { type: "panel"; panel: ReviewPanelDto | null }
  | { type: "submission"; submission: SubmissionDto }
  | { type: "summary"; body: string }
  | { type: "summaryLoaded"; body: string }
  | { type: "panelNotice"; notice: string }
  | { type: "findingConflict"; conflict: FindingConflict }
  | { type: "file"; file: FileDetailDto }
  | { type: "sidebar"; sidebar: SidebarDto }
  | { type: "move"; delta: 1 | -1; extend: boolean }
  | { type: "click"; index: number }
  | { type: "toggleComposer" }
  | { type: "openComposer"; index: number }
  | { type: "closeComposer" }
  | { type: "drafts"; drafts: DraftsDto }
  | { type: "writeFailure"; failure: string | null }
  | { type: "editRejected" };

/** Clamps a cursor into a row range, or to zero when the file has no rows. */
function clampCursor(value: number, rowCount: number): number {
  return rowCount === 0 ? 0 : clamp(value, 0, rowCount - 1);
}

export function sessionReducer(state: SessionState, action: SessionAction): SessionState {
  switch (action.type) {
    case "describe":
      if (state.status !== "loading") {
        return state;
      }
      return { ...state, description: action.description };

    case "stage":
      if (state.status !== "loading") {
        return state;
      }
      return { ...state, stage: action.stage };

    case "failed":
      return { status: "failed", failure: action.failure };

    case "ready":
      return {
        status: "ready",
        snapshot: action.snapshot,
        file: action.file,
        cursor: 0,
        anchor: 0,
        drafts: action.file.drafts,
        composer: null,
        panel: action.panel,
        panelNotice: null,
        findingConflict: null,
        submission: { revision: 0, phase: { state: "Idle" } },
        summary: { body: action.snapshot.summary, loads: 0 },
      };

    case "submission": {
      if (state.status !== "ready") {
        return state;
      }
      // Several submission commands answer with the whole state and they can be
      // in flight at once. One that read the model before a change must not land
      // on top of one that carries it, which would show a confirmation for a
      // verdict the model is no longer holding.
      if (action.submission.revision < state.submission.revision) {
        return state;
      }
      return { ...state, submission: action.submission };
    }

    case "summary":
      if (state.status !== "ready") {
        return state;
      }
      // Typed, so the editor already shows it and must not be handed it back.
      return { ...state, summary: { ...state.summary, body: action.body } };

    case "summaryLoaded":
      if (state.status !== "ready") {
        return state;
      }
      // Replaced by the backend, so the editor is told to take it.
      return {
        ...state,
        summary: { body: action.body, loads: state.summary.loads + 1 },
      };

    case "panel": {
      if (state.status !== "ready") {
        return state;
      }
      // Several commands answer with the whole panel and they run at once. One
      // that read the model before a change must not land on top of one that
      // carries it, which would put a finished run back on screen as a running
      // one with a live Cancel button.
      const stale =
        action.panel !== null &&
        state.panel !== null &&
        action.panel.revision < state.panel.revision;
      if (stale) {
        return state;
      }
      // A finding's id can come to mean a different claim after a re-run, so a
      // pending replace-or-keep confirmation cannot be trusted across any new
      // panel and is dropped along with it.
      return { ...state, panel: action.panel, panelNotice: null, findingConflict: null };
    }

    case "panelNotice":
      if (state.status !== "ready") {
        return state;
      }
      return { ...state, panelNotice: action.notice };

    case "findingConflict":
      if (state.status !== "ready") {
        return state;
      }
      return { ...state, findingConflict: action.conflict };

    case "file":
      if (state.status !== "ready") {
        return state;
      }
      return {
        ...state,
        file: action.file,
        cursor: 0,
        anchor: 0,
        drafts: action.file.drafts,
        composer: null,
      };

    case "sidebar":
      if (state.status !== "ready") {
        return state;
      }
      return { ...state, snapshot: { ...state.snapshot, sidebar: action.sidebar } };

    case "move": {
      if (state.status !== "ready") {
        return state;
      }
      const cursor = clampCursor(state.cursor + action.delta, state.file.rows.length);
      const anchor = action.extend ? state.anchor : cursor;
      return { ...state, cursor, anchor };
    }

    case "click": {
      if (state.status !== "ready") {
        return state;
      }
      const cursor = clampCursor(action.index, state.file.rows.length);
      return { ...state, cursor, anchor: cursor };
    }

    case "toggleComposer": {
      if (state.status !== "ready") {
        return state;
      }
      const [start, end] = selectionRange(state);
      if (state.composer && state.composer.rows[0] === start && state.composer.rows[1] === end) {
        return { ...state, composer: null };
      }
      return { ...state, composer: { rows: [start, end], notice: null } };
    }

    case "openComposer": {
      if (state.status !== "ready") {
        return state;
      }
      const cursor = clampCursor(action.index, state.file.rows.length);
      return { ...state, cursor, anchor: cursor, composer: { rows: [cursor, cursor], notice: null } };
    }

    case "closeComposer":
      if (state.status !== "ready") {
        return state;
      }
      return { ...state, composer: null };

    case "drafts":
      if (state.status !== "ready" || action.drafts.file_index !== state.drafts.file_index) {
        return state;
      }
      return {
        ...state,
        drafts: action.drafts,
        composer: state.composer ? { ...state.composer, notice: null } : null,
      };

    case "writeFailure": {
      if (state.status !== "ready" || state.drafts.write_failure === action.failure) {
        return state;
      }
      // The same session-wide reason a drafts answer carries, from the one
      // command a reviewer writing only a summary ever calls.
      return { ...state, drafts: { ...state.drafts, write_failure: action.failure } };
    }

    case "editRejected":
      if (state.status !== "ready" || !state.composer) {
        return state;
      }
      return { ...state, composer: { ...state.composer, notice: REJECTED_NOTICE } };
  }
}

/** The inclusive row range the reviewer has selected, low index first. */
export function selectionRange(state: ReadyState): [number, number] {
  return [Math.min(state.anchor, state.cursor), Math.max(state.anchor, state.cursor)];
}

/** The draft anchored to a row, if the projection has one there. */
export function draftAtRow(drafts: DraftsDto, row: number): AnchoredDraftDto | undefined {
  return drafts.anchored.find((draft) => draft.row === row);
}

/** The text a freshly opened composer should show, silently, before any typing. */
export function composerPrefill(state: ReadyState): string {
  if (!state.composer) {
    return "";
  }
  return draftAtRow(state.drafts, state.composer.rows[1])?.body ?? "";
}

/** The finding the panel currently has selected, if any. */
export function selectedFinding(panel: ReviewPanelDto | null): FindingDto | null {
  return panel?.findings.find((finding) => finding.is_selected) ?? null;
}
