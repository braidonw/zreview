import type { FileDetailDto, SessionFailureDto, SessionSnapshotDto, SidebarDto } from "../bindings";
import { clamp } from "../lib/clamp";

/** What the loading screen shows before any session request has begun. */
const DEMO_DESCRIPTION = "the generated fixture";

export type SessionState =
  | { status: "loading"; description: string; stage: string }
  | { status: "failed"; failure: SessionFailureDto }
  | {
      status: "ready";
      snapshot: SessionSnapshotDto;
      file: FileDetailDto;
      cursor: number;
      anchor: number;
    };

export type ReadyState = Extract<SessionState, { status: "ready" }>;

export const initialState: SessionState = {
  status: "loading",
  description: DEMO_DESCRIPTION,
  stage: "Starting",
};

export type SessionAction =
  | { type: "stage"; stage: string }
  | { type: "failed"; failure: SessionFailureDto }
  | { type: "ready"; snapshot: SessionSnapshotDto; file: FileDetailDto }
  | { type: "file"; file: FileDetailDto }
  | { type: "sidebar"; sidebar: SidebarDto }
  | { type: "move"; delta: 1 | -1; extend: boolean }
  | { type: "click"; index: number };

/** Clamps a cursor into a row range, or to zero when the file has no rows. */
function clampCursor(value: number, rowCount: number): number {
  return rowCount === 0 ? 0 : clamp(value, 0, rowCount - 1);
}

export function sessionReducer(state: SessionState, action: SessionAction): SessionState {
  switch (action.type) {
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
      };

    case "file":
      if (state.status !== "ready") {
        return state;
      }
      return { ...state, file: action.file, cursor: 0, anchor: 0 };

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
  }
}

/** The inclusive row range the reviewer has selected, low index first. */
export function selectionRange(state: ReadyState): [number, number] {
  return [Math.min(state.anchor, state.cursor), Math.max(state.anchor, state.cursor)];
}
