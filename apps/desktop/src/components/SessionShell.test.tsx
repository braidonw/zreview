import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import type { ReadyState } from "../hooks/sessionReducer";
import {
  makeFile,
  makeFileSummary,
  makeRow,
  makeSidebar,
  makeSnapshot,
  makeSubmission,
} from "../test/fixtures";
import { SessionShell } from "./SessionShell";

/** Every prop SessionShell needs beyond the ones a test wants to vary. */
function baseHandlers() {
  return {
    isShowing: true,
    onBack: null,
    onSelectFile: () => {},
    onRowClick: () => {},
    onOpenComposer: () => {},
    onComposerChange: () => {},
    onComposerClose: () => {},
    onComposerDiscard: () => {},
    onReanchorDraft: () => {},
    onRunReview: () => {},
    onCancelReview: () => {},
    onToggleGuidanceSection: () => {},
    onToggleGuidanceFile: () => {},
    onRevealFinding: () => {},
    onAcceptFinding: () => {},
    onDismissFinding: () => {},
    onReplaceFinding: () => {},
    onKeepFinding: () => {},
    onSummaryChange: () => {},
    onSubmit: () => {},
    onCancelSubmission: () => {},
    onSendSubmission: () => {},
  };
}

function readyState(overrides: Partial<ReadyState["file"]> = {}): ReadyState {
  return {
    status: "ready",
    snapshot: makeSnapshot({ sidebar: makeSidebar([makeFileSummary()]) }),
    file: makeFile({ rows: [makeRow({ text: "a line" })], ...overrides }),
    cursor: 0,
    anchor: 0,
    drafts: makeFile().drafts,
    composer: null,
    panel: null,
    panelNotice: null,
    findingConflict: null,
    submission: makeSubmission({ state: "Idle" }, 0),
    summary: { body: "", loads: 0 },
  };
}

describe("SessionShell", () => {
  it("renders the diff list when the file has rows and no empty reason", () => {
    render(<SessionShell state={readyState()} {...baseHandlers()} />);

    expect(screen.getByText("a line")).toBeTruthy();
  });

  it("shows the empty-diff pane with the domain's reason when the file carries one", () => {
    render(
      <SessionShell
        state={readyState({
          rows: [],
          empty_reason: { label: "Binary file", detail: "ZReview does not render binary content yet." },
        })}
        {...baseHandlers()}
      />,
    );

    expect(screen.getByText("Binary file")).toBeTruthy();
    expect(screen.getByText("ZReview does not render binary content yet.")).toBeTruthy();
    expect(screen.queryByText("a line")).toBeNull();
  });

  it("falls back to a generic empty message when rows are empty with no stated reason", () => {
    render(<SessionShell state={readyState({ rows: [] })} {...baseHandlers()} />);

    expect(screen.getByText("No lines to show")).toBeTruthy();
  });
});
