import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { makeGuidance, makeGuidanceEntry, makePanel } from "../test/fixtures";
import { ReviewPanel } from "./ReviewPanel";

/** Every handler the panel needs beyond the one a test wants to watch. */
function baseHandlers() {
  return {
    notice: null,
    onRunReview: () => {},
    onCancelReview: () => {},
    onToggleGuidanceSection: () => {},
    onToggleGuidanceFile: () => {},
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
});
