import { describe, expect, it } from "vitest";
import type { FileDetailDto, SessionFailureDto } from "../bindings";
import { makeAnchoredDraft, makeDrafts, makeFile, makeRow, makeSnapshot } from "../test/fixtures";
import {
  type ReadyState,
  composerPrefill,
  draftAtRow,
  initialState,
  selectionRange,
  sessionReducer,
} from "./sessionReducer";

function fileWithRows(rowCount: number): FileDetailDto {
  return makeFile({
    rows: Array.from({ length: rowCount }, (_, index) => makeRow({ text: `line ${index}` })),
  });
}

function readyState(rowCount: number, cursor = 0, anchor = 0): ReadyState {
  const file = fileWithRows(rowCount);
  return {
    status: "ready",
    snapshot: makeSnapshot(),
    file,
    cursor,
    anchor,
    drafts: file.drafts,
    composer: null,
  };
}

describe("sessionReducer", () => {
  it("starts on the loading phase with no description yet", () => {
    expect(initialState).toEqual({
      status: "loading",
      description: "",
      stage: "Starting",
    });
  });

  it("fills in the description once the request describes itself", () => {
    const next = sessionReducer(initialState, {
      type: "describe",
      description: "main…HEAD",
    });
    expect(next).toEqual({ ...initialState, description: "main…HEAD" });
  });

  it("ignores a late description once the session is no longer loading", () => {
    const next = sessionReducer(readyState(1), {
      type: "describe",
      description: "main…HEAD",
    });
    expect(next).toEqual(readyState(1));
  });

  it("updates the stage while loading", () => {
    const next = sessionReducer(initialState, { type: "stage", stage: "Fetching objects" });
    expect(next).toEqual({ ...initialState, stage: "Fetching objects" });
  });

  it("clamps the cursor at the top of the file", () => {
    const next = sessionReducer(readyState(10, 0, 0), { type: "move", delta: -1, extend: false });
    expect(next).toMatchObject({ cursor: 0, anchor: 0 });
  });

  it("clamps the cursor at the bottom of the file", () => {
    const next = sessionReducer(readyState(10, 9, 9), { type: "move", delta: 1, extend: false });
    expect(next).toMatchObject({ cursor: 9, anchor: 9 });
  });

  it("moves the cursor and anchor together without extending", () => {
    const next = sessionReducer(readyState(10, 3, 3), { type: "move", delta: 1, extend: false });
    expect(next).toMatchObject({ cursor: 4, anchor: 4 });
  });

  it("grows the range when extending away from the anchor", () => {
    const next = sessionReducer(readyState(10, 3, 3), { type: "move", delta: 1, extend: true });
    expect(next).toMatchObject({ cursor: 4, anchor: 3 });
    expect(selectionRange(next as ReadyState)).toEqual([3, 4]);
  });

  it("shrinks the range when extending back across the anchor", () => {
    const extended = sessionReducer(readyState(10, 3, 1), {
      type: "move",
      delta: 1,
      extend: true,
    });
    expect(extended).toMatchObject({ cursor: 4, anchor: 1 });

    const shrunk = sessionReducer(extended, { type: "move", delta: -1, extend: true });
    expect(shrunk).toMatchObject({ cursor: 3, anchor: 1 });
  });

  it("resets the cursor and anchor when the file switches, closing the composer and swapping drafts", () => {
    const state = sessionReducer(readyState(10, 7, 2), { type: "toggleComposer" }) as ReadyState;
    expect(state.composer).not.toBeNull();

    const nextFile = fileWithRows(5);
    nextFile.drafts = makeDrafts({ file_draft_count: 3 });
    const next = sessionReducer(state, { type: "file", file: nextFile }) as ReadyState;

    expect(next).toMatchObject({ cursor: 0, anchor: 0, composer: null });
    expect(next.file.rows).toHaveLength(5);
    expect(next.drafts.file_draft_count).toBe(3);
  });

  it("collapses the selection to the clicked row", () => {
    const next = sessionReducer(readyState(10, 3, 1), { type: "click", index: 6 });
    expect(next).toMatchObject({ cursor: 6, anchor: 6 });
  });

  it("keeps the cursor at zero and is a no-op on an empty file", () => {
    const next = sessionReducer(readyState(0, 0, 0), { type: "move", delta: 1, extend: false });
    expect(next).toMatchObject({ cursor: 0, anchor: 0 });
  });

  it("keeps the cursor at zero on click for an empty file", () => {
    const next = sessionReducer(readyState(0, 0, 0), { type: "click", index: 5 });
    expect(next).toMatchObject({ cursor: 0, anchor: 0 });
  });

  it("moves to the failed phase with the failure DTO", () => {
    const failure: SessionFailureDto = { summary: "boom", detail: "detail", remediation: "fix it" };
    const next = sessionReducer(initialState, { type: "failed", failure });
    expect(next).toEqual({ status: "failed", failure });
  });

  it("opens the composer on the selected span, frozen at that instant", () => {
    const extended = sessionReducer(readyState(10, 3, 3), {
      type: "move",
      delta: 1,
      extend: true,
    }) as ReadyState;

    const next = sessionReducer(extended, { type: "toggleComposer" }) as ReadyState;

    expect(next.composer).toEqual({ rows: [3, 4], notice: null });
  });

  it("closes the composer when toggled again over the same frozen span", () => {
    const opened = sessionReducer(readyState(10, 3, 3), { type: "toggleComposer" }) as ReadyState;
    expect(opened.composer).not.toBeNull();

    const closed = sessionReducer(opened, { type: "toggleComposer" }) as ReadyState;

    expect(closed.composer).toBeNull();
  });

  it("reopens on the new span when the selection has moved since it was frozen", () => {
    const opened = sessionReducer(readyState(10, 3, 3), { type: "toggleComposer" }) as ReadyState;
    const moved = sessionReducer(opened, { type: "move", delta: 1, extend: false }) as ReadyState;

    const reopened = sessionReducer(moved, { type: "toggleComposer" }) as ReadyState;

    expect(reopened.composer).toEqual({ rows: [4, 4], notice: null });
  });

  it("opens the composer on a clicked pill, moving the cursor there", () => {
    const next = sessionReducer(readyState(10, 0, 0), { type: "openComposer", index: 6 }) as ReadyState;

    expect(next).toMatchObject({ cursor: 6, anchor: 6 });
    expect(next.composer).toEqual({ rows: [6, 6], notice: null });
  });

  it("closes the composer explicitly", () => {
    const opened = sessionReducer(readyState(10, 3, 3), { type: "toggleComposer" }) as ReadyState;

    const next = sessionReducer(opened, { type: "closeComposer" }) as ReadyState;

    expect(next.composer).toBeNull();
  });

  it("ignores a drafts response for a file that is no longer selected", () => {
    const state = readyState(10, 3, 3);
    const stale = makeDrafts({
      file_index: 1,
      anchored: [makeAnchoredDraft({ row: 3, body: "for the wrong file" })],
    });

    const next = sessionReducer(state, { type: "drafts", drafts: stale });

    expect(next).toBe(state);
  });

  it("replaces the drafts projection and clears a stale notice on settle", () => {
    const opened = sessionReducer(readyState(10, 3, 3), { type: "toggleComposer" }) as ReadyState;
    const rejected = sessionReducer(opened, { type: "editRejected" }) as ReadyState;
    expect(rejected.composer?.notice).not.toBeNull();

    const drafts = makeDrafts({ anchored: [makeAnchoredDraft({ row: 3, body: "kept" })] });
    const next = sessionReducer(rejected, { type: "drafts", drafts }) as ReadyState;

    expect(next.drafts).toBe(drafts);
    expect(next.composer?.notice).toBeNull();
  });

  it("shows the rejection notice under the composer only on a false outcome", () => {
    const opened = sessionReducer(readyState(10, 3, 3), { type: "toggleComposer" }) as ReadyState;

    const next = sessionReducer(opened, { type: "editRejected" }) as ReadyState;

    expect(next.composer?.notice).toBe("This selection cannot hold a comment");
  });

  it("ignores editRejected when the composer is closed", () => {
    const next = sessionReducer(readyState(10), { type: "editRejected" }) as ReadyState;
    expect(next.composer).toBeNull();
  });

  it("finds the draft anchored to a row", () => {
    const drafts = makeDrafts({ anchored: [makeAnchoredDraft({ row: 4, body: "hello" })] });

    expect(draftAtRow(drafts, 4)?.body).toBe("hello");
    expect(draftAtRow(drafts, 5)).toBeUndefined();
  });

  it("prefills the composer silently from the draft at its span's end row", () => {
    const drafts = makeDrafts({ anchored: [makeAnchoredDraft({ row: 4, body: "existing" })] });
    const withDrafts = { ...readyState(10), drafts } as ReadyState;
    const opened = sessionReducer(withDrafts, { type: "openComposer", index: 4 }) as ReadyState;

    expect(composerPrefill(opened)).toBe("existing");
  });

  it("prefills empty when the row has no draft or the composer is closed", () => {
    expect(composerPrefill(readyState(10))).toBe("");
    const opened = sessionReducer(readyState(10, 2, 2), { type: "toggleComposer" }) as ReadyState;
    expect(composerPrefill(opened)).toBe("");
  });
});
