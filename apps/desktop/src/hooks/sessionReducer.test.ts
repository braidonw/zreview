import { describe, expect, it } from "vitest";
import type { FileDetailDto, SessionFailureDto } from "../bindings";
import { makeFile, makeRow, makeSnapshot } from "../test/fixtures";
import {
  type ReadyState,
  type SessionState,
  initialState,
  selectionRange,
  sessionReducer,
} from "./sessionReducer";

function fileWithRows(rowCount: number): FileDetailDto {
  return makeFile({
    rows: Array.from({ length: rowCount }, (_, index) => makeRow({ text: `line ${index}` })),
  });
}

function readyState(rowCount: number, cursor = 0, anchor = 0): SessionState {
  return { status: "ready", snapshot: makeSnapshot(), file: fileWithRows(rowCount), cursor, anchor };
}

describe("sessionReducer", () => {
  it("starts on the loading phase with the demo description", () => {
    expect(initialState).toEqual({
      status: "loading",
      description: "the generated fixture",
      stage: "Starting",
    });
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

  it("resets the cursor and anchor when the file switches", () => {
    const next = sessionReducer(readyState(10, 7, 2), { type: "file", file: fileWithRows(5) });
    expect(next).toMatchObject({ cursor: 0, anchor: 0 });
    expect((next as ReadyState).file.rows).toHaveLength(5);
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
});
