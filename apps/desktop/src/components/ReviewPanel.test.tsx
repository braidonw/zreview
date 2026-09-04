import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { makeFinding, makeGuidance, makeGuidanceEntry, makePanel } from "../test/fixtures";
import { ReviewPanel } from "./ReviewPanel";

/** Every handler the panel needs beyond the one a test wants to watch. */
function baseHandlers() {
  return {
    notice: null,
    findingConflict: null,
    onRunReview: () => {},
    onCancelReview: () => {},
    onToggleGuidanceSection: () => {},
    onToggleGuidanceFile: () => {},
    onRevealFinding: () => {},
    onAcceptFinding: () => {},
    onDismissFinding: () => {},
    onReplaceFinding: () => {},
    onKeepFinding: () => {},
  };
}

describe("ReviewPanel", () => {
  it("lists every discovered file with what it applies to and how big it is", () => {
    render(
      <ReviewPanel
        panel={makePanel({
          guidance: makeGuidance({
            entries: [
              makeGuidanceEntry({ path: "AGENTS.md", scope: "whole repository", kilobytes: 2 }),
              makeGuidanceEntry({ path: "src/AGENTS.md", scope: "src/", kilobytes: 1 }),
            ],
            summary: "2 guidance files · 3 KB",
          }),
        })}
        {...baseHandlers()}
      />,
    );

    expect(screen.getByText("2 guidance files · 3 KB")).toBeTruthy();
    expect(screen.getByText("AGENTS.md")).toBeTruthy();
    expect(screen.getByText("whole repository · 2K")).toBeTruthy();
    expect(screen.getByText("src/AGENTS.md")).toBeTruthy();
    expect(screen.getByText("src/ · 1K")).toBeTruthy();
  });

  it("shows what discovery skipped with the reason, and what config keeps out of the review", () => {
    render(
      <ReviewPanel
        panel={makePanel({
          guidance: makeGuidance({
            skipped: [{ path: "CLAUDE.md", reason: "90000 bytes, over the 65536-byte limit" }],
            excluded: "1 file excluded from review by .zreview.toml",
          }),
        })}
        {...baseHandlers()}
      />,
    );

    expect(screen.getByText("CLAUDE.md")).toBeTruthy();
    expect(screen.getByText("90000 bytes, over the 65536-byte limit")).toBeTruthy();
    expect(screen.getByText("1 file excluded from review by .zreview.toml")).toBeTruthy();
  });

  it("says plainly when discovery found nothing", () => {
    render(
      <ReviewPanel
        panel={makePanel({
          guidance: {
            kind: "NothingFound",
            note: "No guidance files found. The review will judge the diff alone.",
          },
        })}
        {...baseHandlers()}
      />,
    );

    expect(
      screen.getByText("No guidance files found. The review will judge the diff alone."),
    ).toBeTruthy();
  });

  it("keeps the summary line but hides the files while the section is collapsed", async () => {
    const onToggleGuidanceSection = vi.fn();
    render(
      <ReviewPanel
        panel={makePanel({ guidance: makeGuidance({ expanded: false }) })}
        {...baseHandlers()}
        onToggleGuidanceSection={onToggleGuidanceSection}
      />,
    );

    expect(screen.getByText("1 guidance file · 2 KB")).toBeTruthy();
    expect(screen.queryByText("AGENTS.md")).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: /1 guidance file/ }));

    expect(onToggleGuidanceSection).toHaveBeenCalledOnce();
  });

  it("hands back the path of the guidance file whose toggle was pressed", async () => {
    const onToggleGuidanceFile = vi.fn();
    render(
      <ReviewPanel
        panel={makePanel()}
        {...baseHandlers()}
        onToggleGuidanceFile={onToggleGuidanceFile}
      />,
    );

    const toggle = screen.getByRole("button", { name: /AGENTS\.md/ });
    expect(toggle.getAttribute("aria-pressed")).toBe("true");

    await userEvent.click(toggle);

    expect(onToggleGuidanceFile).toHaveBeenCalledWith("AGENTS.md");
  });

  it("offers Review while nothing is running, and says a run has not happened", () => {
    render(<ReviewPanel panel={makePanel()} {...baseHandlers()} />);

    expect(screen.getByRole("button", { name: "Review" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();
    expect(screen.getByText("No review has been run.")).toBeTruthy();
  });

  it("shows the backend's latest line and a Cancel control while a run is in flight", () => {
    render(
      <ReviewPanel
        panel={makePanel({
          run: { state: "Running", detail: "Reading the diff" },
          note: { heading: "Reviewing...", detail: null },
        })}
        {...baseHandlers()}
      />,
    );

    expect(screen.getByText("Reading the diff")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Review" })).toBeNull();
  });

  it("shows a failed run's summary and what to do about it", () => {
    render(
      <ReviewPanel
        panel={makePanel({
          run: {
            state: "Failed",
            summary: "claude is not installed",
            remediation: "Install claude and make sure it is on your PATH.",
          },
          note: {
            heading: "claude is not installed",
            detail: "Install claude and make sure it is on your PATH.",
          },
        })}
        {...baseHandlers()}
      />,
    );

    expect(screen.getByText("claude is not installed")).toBeTruthy();
    expect(screen.getByText("Install claude and make sure it is on your PATH.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Review" })).toBeTruthy();
  });

  it("says so when the run the reviewer cancelled ends", () => {
    render(
      <ReviewPanel
        panel={makePanel({
          run: { state: "Failed", summary: "the review was cancelled", remediation: null },
          note: { heading: "the review was cancelled", detail: null },
        })}
        {...baseHandlers()}
      />,
    );

    expect(screen.getByText("the review was cancelled")).toBeTruthy();
  });

  it("shows a completed run's counts and names the files it did not see", () => {
    render(
      <ReviewPanel
        panel={makePanel({
          guidance: makeGuidance({ expanded: false }),
          run: {
            state: "Complete",
            accepted: 0,
            rejected: 2,
            suppressed: 1,
          },
          note: {
            heading: "Nothing to act on.",
            detail: "2 claim(s) did not check out and 1 were previously dismissed.",
          },
          footer: {
            refused: "2 claim(s) refused",
            not_reviewed: "2 file(s) not reviewed",
            unreviewed: ["vendor/lib.rs", "huge.json"],
          },
        })}
        {...baseHandlers()}
      />,
    );

    expect(screen.getByText("Nothing to act on.")).toBeTruthy();
    expect(
      screen.getByText("2 claim(s) did not check out and 1 were previously dismissed."),
    ).toBeTruthy();
    expect(screen.getByText("2 claim(s) refused")).toBeTruthy();
    expect(screen.getByText("2 file(s) not reviewed")).toBeTruthy();
    expect(screen.getByText("vendor/lib.rs")).toBeTruthy();
    expect(screen.getByText("huge.json")).toBeTruthy();
  });

  it("renders a finding with its severity, confidence, position, rationale, citations, and who proposed it", () => {
    render(
      <ReviewPanel
        panel={makePanel({
          findings: [
            makeFinding({
              severity: "Error",
              confidence_percent: 82,
              title: "Unchecked index",
              rationale: "This can panic on an empty slice.",
              citations: ["AGENTS.md", "src/AGENTS.md"],
              origin: "claude-code",
              position: "src/review.rs:6",
            }),
          ],
        })}
        {...baseHandlers()}
      />,
    );

    expect(screen.getByText("error")).toBeTruthy();
    expect(screen.getByText("82%")).toBeTruthy();
    expect(screen.getByText("src/review.rs:6")).toBeTruthy();
    expect(screen.getByText("Unchecked index")).toBeTruthy();
    expect(screen.getByText("This can panic on an empty slice.")).toBeTruthy();
    expect(screen.getByText("per AGENTS.md, src/AGENTS.md")).toBeTruthy();
    expect(screen.getByText("Proposed by claude-code")).toBeTruthy();
    // The note is what the empty list shows; a finding gives way to the list.
    expect(screen.queryByText("No review has been run.")).toBeNull();
  });

  it("shows a finding about the whole change as such, offering no Accept", () => {
    render(
      <ReviewPanel
        panel={makePanel({ findings: [makeFinding({ position: null, title: "no tests anywhere" })] })}
        {...baseHandlers()}
      />,
    );

    expect(screen.getByText("whole change")).toBeTruthy();
    // The desktop has no summary editor yet, so accepting a whole-change
    // finding would write to storage the reviewer would never see.
    expect(screen.queryByRole("button", { name: "Accept" })).toBeNull();
    expect(screen.getByRole("button", { name: "Dismiss" })).toBeTruthy();
  });

  it("highlights the finding the model has selected", () => {
    render(
      <ReviewPanel
        panel={makePanel({
          findings: [
            makeFinding({ id: 1, title: "first", is_selected: false }),
            makeFinding({ id: 2, title: "second", is_selected: true }),
          ],
        })}
        {...baseHandlers()}
      />,
    );

    const selected = screen.getByText("second").closest("li");
    expect(selected?.getAttribute("aria-selected")).toBe("true");
    const unselected = screen.getByText("first").closest("li");
    expect(unselected?.getAttribute("aria-selected")).toBe("false");
  });

  it("reveals a finding when its card is clicked", async () => {
    const onRevealFinding = vi.fn();
    render(
      <ReviewPanel
        panel={makePanel({ findings: [makeFinding({ id: 5, title: "unchecked index" })] })}
        {...baseHandlers()}
        onRevealFinding={onRevealFinding}
      />,
    );

    await userEvent.click(screen.getByText("unchecked index"));

    expect(onRevealFinding).toHaveBeenCalledWith(5);
  });

  it("accepts and dismisses a finding through its own buttons, without also revealing it", async () => {
    const onRevealFinding = vi.fn();
    const onAcceptFinding = vi.fn();
    const onDismissFinding = vi.fn();
    render(
      <ReviewPanel
        panel={makePanel({ findings: [makeFinding({ id: 5 })] })}
        {...baseHandlers()}
        onRevealFinding={onRevealFinding}
        onAcceptFinding={onAcceptFinding}
        onDismissFinding={onDismissFinding}
      />,
    );

    await userEvent.click(screen.getByRole("button", { name: "Accept" }));
    await userEvent.click(screen.getByRole("button", { name: "Dismiss" }));

    expect(onAcceptFinding).toHaveBeenCalledWith(5);
    expect(onDismissFinding).toHaveBeenCalledWith(5);
    expect(onRevealFinding).not.toHaveBeenCalled();
  });

  it("asks whether to replace before overwriting an occupied line", () => {
    render(
      <ReviewPanel
        panel={makePanel({ findings: [makeFinding({ id: 5 })] })}
        {...baseHandlers()}
        findingConflict={{ id: 5, existing: "mine", proposed: "Handle the failure here." }}
      />,
    );

    expect(screen.getByText("Replace your comment with this proposal?")).toBeTruthy();
    // Both texts are on screen, so nobody replaces words they cannot see.
    expect(screen.getByText("mine")).toBeTruthy();
    expect(screen.getByText("Handle the failure here.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Replace" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Keep" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Accept" })).toBeNull();
  });

  it("only asks about the finding the conflict names", () => {
    render(
      <ReviewPanel
        panel={makePanel({
          findings: [makeFinding({ id: 5, title: "first" }), makeFinding({ id: 6, title: "second" })],
        })}
        {...baseHandlers()}
        findingConflict={{ id: 5, existing: "mine", proposed: "Handle the failure here." }}
      />,
    );

    expect(screen.getAllByRole("button", { name: "Accept" })).toHaveLength(1);
    expect(screen.getAllByRole("button", { name: "Replace" })).toHaveLength(1);
  });

  it("replaces or keeps through the confirmation's two buttons", async () => {
    const onReplaceFinding = vi.fn();
    const onKeepFinding = vi.fn();
    render(
      <ReviewPanel
        panel={makePanel({ findings: [makeFinding({ id: 5 })] })}
        {...baseHandlers()}
        findingConflict={{ id: 5, existing: "mine", proposed: "Handle the failure here." }}
        onReplaceFinding={onReplaceFinding}
        onKeepFinding={onKeepFinding}
      />,
    );

    await userEvent.click(screen.getByRole("button", { name: "Replace" }));
    expect(onReplaceFinding).toHaveBeenCalledWith(5);

    await userEvent.click(screen.getByRole("button", { name: "Keep" }));
    expect(onKeepFinding).toHaveBeenCalledOnce();
  });

  it("keeps the rejected count and unreviewed files visible even though findings exist", () => {
    render(
      <ReviewPanel
        panel={makePanel({
          findings: [makeFinding()],
          footer: {
            refused: "1 claim(s) refused",
            not_reviewed: "1 file(s) not reviewed",
            unreviewed: ["vendor/lib.rs"],
          },
        })}
        {...baseHandlers()}
      />,
    );

    expect(screen.getByText("Unchecked index")).toBeTruthy();
    expect(screen.getByText("1 claim(s) refused")).toBeTruthy();
    expect(screen.getByText("1 file(s) not reviewed")).toBeTruthy();
    expect(screen.getByText("vendor/lib.rs")).toBeTruthy();
  });
});
