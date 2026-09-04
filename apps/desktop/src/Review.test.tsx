import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { Channel } from "@tauri-apps/api/core";
import type { FindingDto, ReviewPanelDto } from "./bindings";
import App from "./App";
import {
  makeAnchoredDraft,
  makeDrafts,
  makeFile,
  makeFileSummary,
  makeFinding,
  makeGuidance,
  makeGuidanceEntry,
  makePanel,
  makeRow,
  makeSidebar,
  makeSnapshot,
} from "./test/fixtures";

const describeWindow = vi.fn();
const openSession = vi.fn();
const selectFile = vi.fn();
const toggleViewed = vi.fn();
const editDraft = vi.fn();
const discardDraft = vi.fn();
const reanchorDraft = vi.fn();
const reviewPanel = vi.fn();
const runReview = vi.fn();
const cancelReview = vi.fn();
const toggleGuidancePanel = vi.fn();
const toggleGuidance = vi.fn();
const acceptFinding = vi.fn();
const overwriteFinding = vi.fn();
const dismissFinding = vi.fn();
const revealFinding = vi.fn();
const selectNextFinding = vi.fn();

vi.mock("./bindings", () => ({
  commands: {
    describeWindow: () => describeWindow(),
    openSession: (channel: unknown) => openSession(channel),
    selectFile: (index: unknown) => selectFile(index),
    toggleViewed: () => toggleViewed(),
    editDraft: (...args: unknown[]) => editDraft(...args),
    discardDraft: (...args: unknown[]) => discardDraft(...args),
    reanchorDraft: (...args: unknown[]) => reanchorDraft(...args),
    reviewPanel: () => reviewPanel(),
    runReview: (channel: unknown) => runReview(channel),
    cancelReview: () => cancelReview(),
    toggleGuidancePanel: () => toggleGuidancePanel(),
    toggleGuidance: (path: unknown) => toggleGuidance(path),
    acceptFinding: (id: unknown) => acceptFinding(id),
    overwriteFinding: (id: unknown) => overwriteFinding(id),
    dismissFinding: (id: unknown) => dismissFinding(id),
    revealFinding: (id: unknown) => revealFinding(id),
    selectNextFinding: () => selectNextFinding(),
  },
}));

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: () => Promise.resolve() }));

/** The panel the backend reports while a run is in flight. */
function runningPanel(detail: string, revision = 2): ReviewPanelDto {
  return makePanel({
    revision,
    run: { state: "Running", detail },
    note: { heading: "Reviewing...", detail: null },
  });
}

beforeEach(() => {
  describeWindow.mockReset();
  describeWindow.mockResolvedValue({
    Session: { session: { description: "the generated fixture", row_identity: null } },
  });
  openSession.mockReset();
  openSession.mockResolvedValue({
    status: "ok",
    data: makeSnapshot({ sidebar: makeSidebar([makeFileSummary()]) }),
  });
  selectFile.mockReset();
  selectFile.mockResolvedValue({
    status: "ok",
    data: makeFile({ rows: [makeRow({ text: "first" }), makeRow({ text: "second" })] }),
  });
  toggleViewed.mockReset();
  editDraft.mockReset();
  discardDraft.mockReset();
  reanchorDraft.mockReset();
  reviewPanel.mockReset();
  reviewPanel.mockResolvedValue({ status: "ok", data: makePanel() });
  runReview.mockReset();
  runReview.mockReturnValue(new Promise(() => {}));
  cancelReview.mockReset();
  toggleGuidancePanel.mockReset();
  toggleGuidance.mockReset();
  acceptFinding.mockReset();
  overwriteFinding.mockReset();
  dismissFinding.mockReset();
  revealFinding.mockReset();
  selectNextFinding.mockReset();
});

/** Renders the Session and waits for its panel to appear. */
async function openPanel() {
  render(<App />);
  await waitFor(() => expect(screen.getByRole("button", { name: "Review" })).toBeTruthy());
}

describe("the review panel in a Session", () => {
  it("shows the guidance the session discovered as soon as it opens", async () => {
    await openPanel();

    expect(screen.getByText("1 guidance file · 2 KB")).toBeTruthy();
    expect(screen.getByText("AGENTS.md")).toBeTruthy();
    expect(screen.getByText("No review has been run.")).toBeTruthy();
  });

  it("shows no panel for a snapshot that cannot be reviewed", async () => {
    reviewPanel.mockResolvedValue({ status: "ok", data: null });

    render(<App />);
    await waitFor(() => expect(screen.getByText("first")).toBeTruthy());

    expect(screen.queryByRole("button", { name: "Review" })).toBeNull();
  });

  it("updates the summary line when a guidance file is turned off", async () => {
    const user = userEvent.setup();
    toggleGuidance.mockResolvedValue({
      status: "ok",
      data: makePanel({
        guidance: makeGuidance({
          summary: "No guidance will be sent",
          entries: [makeGuidanceEntry({ included: false })],
        }),
      }),
    });
    await openPanel();

    await user.click(screen.getByRole("button", { name: /AGENTS\.md/ }));

    expect(toggleGuidance).toHaveBeenCalledWith("AGENTS.md");
    await waitFor(() => expect(screen.getByText("No guidance will be sent")).toBeTruthy());
    expect(screen.getByRole("button", { name: /AGENTS\.md/ }).getAttribute("aria-pressed")).toBe(
      "false",
    );
  });

  it("opens and closes the guidance section on the summary line", async () => {
    const user = userEvent.setup();
    toggleGuidancePanel.mockResolvedValue({
      status: "ok",
      data: makePanel({ guidance: makeGuidance({ expanded: false }) }),
    });
    await openPanel();

    await user.click(screen.getByRole("button", { name: /1 guidance file/ }));

    await waitFor(() => expect(screen.queryByText("AGENTS.md")).toBeNull());
    expect(screen.getByText("1 guidance file · 2 KB")).toBeTruthy();
  });

  it("shows what a refused toggle said, leaving the diff and the composer alone", async () => {
    const user = userEvent.setup();
    selectFile.mockResolvedValue({
      status: "ok",
      data: makeFile({
        rows: [makeRow({ text: "first" }), makeRow({ text: "second" })],
        drafts: makeDrafts({ anchored: [makeAnchoredDraft({ row: 0, body: "worth keeping" })] }),
      }),
    });
    toggleGuidance.mockResolvedValue({
      status: "error",
      error: { summary: "no guidance file at AGENTS.md", detail: null, remediation: null },
    });
    await openPanel();
    await user.keyboard("c");
    await waitFor(() =>
      expect(document.querySelector("[data-composer] .cm-content")?.textContent).toContain(
        "worth keeping",
      ),
    );

    await user.click(screen.getByRole("button", { name: /AGENTS\.md/ }));

    await waitFor(() => expect(screen.getByText("no guidance file at AGENTS.md")).toBeTruthy());
    expect(screen.getByText("first")).toBeTruthy();
    expect(document.querySelector("[data-composer] .cm-content")?.textContent).toContain(
      "worth keeping",
    );
  });

  it("shows a review command's transport rejection in the panel rather than losing the Session", async () => {
    const user = userEvent.setup();
    toggleGuidancePanel.mockRejectedValue(new Error("IPC is unavailable"));
    await openPanel();

    await user.click(screen.getByRole("button", { name: /1 guidance file/ }));

    await waitFor(() => expect(screen.getByText("Error: IPC is unavailable")).toBeTruthy());
    expect(screen.getByText("first")).toBeTruthy();
  });

  it("clears the notice once a command answers with a panel again", async () => {
    const user = userEvent.setup();
    toggleGuidance.mockResolvedValue({
      status: "error",
      error: { summary: "no guidance file at AGENTS.md", detail: null, remediation: null },
    });
    toggleGuidancePanel.mockResolvedValue({
      status: "ok",
      data: makePanel({ revision: 2, guidance: makeGuidance({ expanded: false }) }),
    });
    await openPanel();

    await user.click(screen.getByRole("button", { name: /AGENTS\.md/ }));
    await waitFor(() => expect(screen.getByText("no guidance file at AGENTS.md")).toBeTruthy());

    await user.click(screen.getByRole("button", { name: /1 guidance file/ }));

    await waitFor(() => expect(screen.queryByText("no guidance file at AGENTS.md")).toBeNull());
  });

  it("drops a command's answer that was read before the run had finished", async () => {
    const user = userEvent.setup();
    let channel: Channel<ReviewPanelDto> | undefined;
    let settleRun: (outcome: unknown) => void = () => {};
    let settleCancel: (outcome: unknown) => void = () => {};
    runReview.mockImplementation((given: Channel<ReviewPanelDto>) => {
      channel = given;
      return new Promise((resolve) => {
        settleRun = resolve;
      });
    });
    cancelReview.mockImplementation(
      () =>
        new Promise((resolve) => {
          settleCancel = resolve;
        }),
    );
    await openPanel();

    await user.click(screen.getByRole("button", { name: "Review" }));
    act(() => channel?.onmessage(runningPanel("Reading the diff", 3)));
    await waitFor(() => expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy());
    await user.click(screen.getByRole("button", { name: "Cancel" }));

    await act(async () => {
      settleRun({
        status: "ok",
        data: makePanel({
          revision: 9,
          run: { state: "Complete", accepted: 0, rejected: 0, suppressed: 0 },
          note: { heading: "Nothing to act on.", detail: "The review found no problems." },
        }),
      });
    });
    // Cancel read the model while the run was still going, and answers last.
    await act(async () => {
      settleCancel({ status: "ok", data: runningPanel("Reading the diff", 3) });
    });

    expect(screen.getByText("Nothing to act on.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();
  });

  it("starts one run when Review is pressed twice before the backend has answered", async () => {
    const user = userEvent.setup();
    await openPanel();

    await user.click(screen.getByRole("button", { name: "Review" }));
    await user.click(screen.getByRole("button", { name: "Review" }));

    expect(runReview).toHaveBeenCalledOnce();
  });

  it("starts a run on cmd-shift-R, and a second trigger during it starts nothing", async () => {
    const user = userEvent.setup();
    let channel: Channel<ReviewPanelDto> | undefined;
    runReview.mockImplementation((given: Channel<ReviewPanelDto>) => {
      channel = given;
      return new Promise(() => {});
    });
    await openPanel();

    await user.keyboard("{Meta>}{Shift>}r{/Shift}{/Meta}");
    expect(runReview).toHaveBeenCalledOnce();

    act(() => channel?.onmessage(runningPanel("Starting...")));
    await waitFor(() => expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy());

    await user.keyboard("{Meta>}{Shift>}r{/Shift}{/Meta}");

    expect(runReview).toHaveBeenCalledOnce();
  });

  it("leaves cmd-shift-R to the composer while one has focus", async () => {
    const user = userEvent.setup();
    await openPanel();

    await user.keyboard("c");
    const editorContent = await waitFor(() => {
      const element = document.querySelector("[data-composer] .cm-content");
      if (!element) {
        throw new Error("composer editor not mounted yet");
      }
      return element;
    });

    editorContent.dispatchEvent(
      new KeyboardEvent("keydown", { key: "r", metaKey: true, shiftKey: true, bubbles: true }),
    );

    expect(runReview).not.toHaveBeenCalled();
  });

  it("shows each progress line the backend reports", async () => {
    const user = userEvent.setup();
    let channel: Channel<ReviewPanelDto> | undefined;
    runReview.mockImplementation((given: Channel<ReviewPanelDto>) => {
      channel = given;
      return new Promise(() => {});
    });
    await openPanel();

    await user.click(screen.getByRole("button", { name: "Review" }));

    act(() => channel?.onmessage(runningPanel("Starting claude")));
    await waitFor(() => expect(screen.getByText("Starting claude")).toBeTruthy());

    act(() => channel?.onmessage(runningPanel("Checking 3 findings against the diff")));
    await waitFor(() =>
      expect(screen.getByText("Checking 3 findings against the diff")).toBeTruthy(),
    );
    expect(screen.queryByText("Starting claude")).toBeNull();
  });

  it("asks the run to stop on Cancel, and says so once it ends", async () => {
    const user = userEvent.setup();
    let channel: Channel<ReviewPanelDto> | undefined;
    let settle: (outcome: unknown) => void = () => {};
    runReview.mockImplementation((given: Channel<ReviewPanelDto>) => {
      channel = given;
      return new Promise((resolve) => {
        settle = resolve;
      });
    });
    cancelReview.mockResolvedValue({ status: "ok", data: runningPanel("Reading the diff") });
    await openPanel();

    await user.click(screen.getByRole("button", { name: "Review" }));
    act(() => channel?.onmessage(runningPanel("Reading the diff")));
    await waitFor(() => expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy());

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(cancelReview).toHaveBeenCalledOnce();

    await act(async () => {
      settle({
        status: "ok",
        data: makePanel({
          revision: 4,
          run: { state: "Failed", summary: "the review was cancelled", remediation: null },
          note: { heading: "the review was cancelled", detail: null },
        }),
      });
    });

    expect(screen.getByText("the review was cancelled")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Review" })).toBeTruthy();
  });

  it("shows a failed run's summary and what to do about it", async () => {
    const user = userEvent.setup();
    runReview.mockResolvedValue({
      status: "ok",
      data: makePanel({
        run: {
          state: "Failed",
          summary: "claude is not installed",
          remediation: "Install claude and make sure it is on your PATH.",
        },
        note: {
          heading: "claude is not installed",
          detail: "Install claude and make sure it is on your PATH.",
        },
      }),
    });
    await openPanel();

    await user.click(screen.getByRole("button", { name: "Review" }));

    await waitFor(() => expect(screen.getByText("claude is not installed")).toBeTruthy());
    expect(screen.getByText("Install claude and make sure it is on your PATH.")).toBeTruthy();
  });

  it("shows a completed run's counts and the files it did not see, guidance collapsed", async () => {
    const user = userEvent.setup();
    runReview.mockResolvedValue({
      status: "ok",
      data: makePanel({
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
          not_reviewed: "1 file(s) not reviewed",
          unreviewed: ["vendor/lib.rs"],
        },
      }),
    });
    await openPanel();

    await user.click(screen.getByRole("button", { name: "Review" }));

    await waitFor(() => expect(screen.getByText("Nothing to act on.")).toBeTruthy());
    expect(
      screen.getByText("2 claim(s) did not check out and 1 were previously dismissed."),
    ).toBeTruthy();
    expect(screen.getByText("2 claim(s) refused")).toBeTruthy();
    expect(screen.getByText("1 file(s) not reviewed")).toBeTruthy();
    expect(screen.getByText("vendor/lib.rs")).toBeTruthy();
    // The disclosure has served its purpose; the summary line stays either way.
    expect(screen.queryByText("AGENTS.md")).toBeNull();
    expect(screen.getByText("1 guidance file · 2 KB")).toBeTruthy();
  });

  it("keeps the rejected count and unreviewed files on screen once a run also finds findings", async () => {
    const user = userEvent.setup();
    runReview.mockResolvedValue({
      status: "ok",
      data: makePanel({
        findings: [makeFinding({ id: 1 })],
        run: { state: "Complete", accepted: 1, rejected: 1, suppressed: 0 },
        footer: {
          refused: "1 claim(s) refused",
          not_reviewed: "1 file(s) not reviewed",
          unreviewed: ["vendor/lib.rs"],
        },
      }),
    });
    await openPanel();

    await user.click(screen.getByRole("button", { name: "Review" }));

    await waitFor(() => expect(screen.getByText("Unchecked index")).toBeTruthy());
    expect(screen.getByText("1 claim(s) refused")).toBeTruthy();
    expect(screen.getByText("1 file(s) not reviewed")).toBeTruthy();
    expect(screen.getByText("vendor/lib.rs")).toBeTruthy();
  });
});

describe("findings in the review panel", () => {
  /** A panel already showing one finding, selected. */
  function panelWithFinding(overrides: Partial<FindingDto> = {}) {
    return makePanel({ findings: [makeFinding({ id: 7, is_selected: true, ...overrides })] });
  }

  it("clicking a finding reveals it, scrolling the diff and selecting the row", async () => {
    const user = userEvent.setup();
    reviewPanel.mockResolvedValue({ status: "ok", data: panelWithFinding() });
    revealFinding.mockResolvedValue({
      status: "ok",
      data: { panel: panelWithFinding(), location: { file: 0, row: 1 } },
    });
    await openPanel();

    await user.click(screen.getByText("Unchecked index"));

    expect(revealFinding).toHaveBeenCalledWith(7);
    await waitFor(() =>
      expect(screen.getByText("second").closest(".diff-row")?.className).toContain(
        "diff-row--selected",
      ),
    );
  });

  it("cmd-shift-F selects the next finding", async () => {
    const user = userEvent.setup();
    reviewPanel.mockResolvedValue({ status: "ok", data: panelWithFinding() });
    selectNextFinding.mockResolvedValue({
      status: "ok",
      data: { panel: panelWithFinding(), location: null },
    });
    await openPanel();

    await user.keyboard("{Meta>}{Shift>}f{/Shift}{/Meta}");

    expect(selectNextFinding).toHaveBeenCalledOnce();
  });

  it("cmd-shift-Y accepts the selected finding", async () => {
    const user = userEvent.setup();
    reviewPanel.mockResolvedValue({ status: "ok", data: panelWithFinding() });
    acceptFinding.mockResolvedValue({
      status: "ok",
      data: { panel: makePanel({ findings: [] }), drafts: makeDrafts(), disposition: { outcome: "Drafted" } },
    });
    await openPanel();

    await user.keyboard("{Meta>}{Shift>}y{/Shift}{/Meta}");

    expect(acceptFinding).toHaveBeenCalledWith(7);
  });

  it("cmd-shift-D dismisses the selected finding", async () => {
    const user = userEvent.setup();
    reviewPanel.mockResolvedValue({ status: "ok", data: panelWithFinding() });
    dismissFinding.mockResolvedValue({ status: "ok", data: makePanel({ findings: [] }) });
    await openPanel();

    await user.keyboard("{Meta>}{Shift>}d{/Shift}{/Meta}");

    expect(dismissFinding).toHaveBeenCalledWith(7);
  });

  it("does nothing on the finding shortcuts while the composer has focus", async () => {
    const user = userEvent.setup();
    reviewPanel.mockResolvedValue({ status: "ok", data: panelWithFinding() });
    await openPanel();

    await user.keyboard("c");
    const editorContent = await waitFor(() => {
      const element = document.querySelector("[data-composer] .cm-content");
      if (!element) {
        throw new Error("composer editor not mounted yet");
      }
      return element;
    });

    editorContent.dispatchEvent(
      new KeyboardEvent("keydown", { key: "y", metaKey: true, shiftKey: true, bubbles: true }),
    );

    expect(acceptFinding).not.toHaveBeenCalled();
  });

  it("asks whether to replace, then replaces the reviewer's draft on confirmation", async () => {
    const user = userEvent.setup();
    reviewPanel.mockResolvedValue({ status: "ok", data: panelWithFinding() });
    acceptFinding.mockResolvedValue({
      status: "ok",
      data: {
        panel: panelWithFinding(),
        drafts: makeDrafts(),
        disposition: { outcome: "Occupied", existing: "mine", proposed: "Handle the failure here." },
      },
    });
    overwriteFinding.mockResolvedValue({
      status: "ok",
      data: {
        panel: makePanel({ findings: [] }),
        drafts: makeDrafts({
          anchored: [makeAnchoredDraft({ row: 0, body: "Handle the failure here.", is_proposed: true })],
        }),
        disposition: { outcome: "Drafted" },
      },
    });
    await openPanel();

    await user.click(screen.getByRole("button", { name: "Accept" }));
    await waitFor(() =>
      expect(
        screen.getByText("This line already has your comment. Replace it with the proposal?"),
      ).toBeTruthy(),
    );

    await user.click(screen.getByRole("button", { name: "Replace" }));

    expect(overwriteFinding).toHaveBeenCalledWith(7);
    await waitFor(() => expect(screen.queryByText("Unchecked index")).toBeNull());
  });

  it("keeping leaves the reviewer's draft and the finding both untouched", async () => {
    const user = userEvent.setup();
    reviewPanel.mockResolvedValue({ status: "ok", data: panelWithFinding() });
    acceptFinding.mockResolvedValue({
      status: "ok",
      data: {
        panel: panelWithFinding(),
        drafts: makeDrafts(),
        disposition: { outcome: "Occupied", existing: "mine", proposed: "Handle the failure here." },
      },
    });
    await openPanel();

    await user.click(screen.getByRole("button", { name: "Accept" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Keep" })).toBeTruthy());

    await user.click(screen.getByRole("button", { name: "Keep" }));

    expect(overwriteFinding).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.queryByRole("button", { name: "Keep" })).toBeNull());
    expect(screen.getByText("Unchecked index")).toBeTruthy();
  });
});
