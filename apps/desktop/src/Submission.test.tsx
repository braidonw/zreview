import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import type { Channel } from "@tauri-apps/api/core";
import type { SubmissionDto } from "./bindings";
import userEvent from "@testing-library/user-event";
import App from "./App";
import {
  makeDrafts,
  makeFile,
  makeFileSummary,
  makeFinding,
  makePanel,
  makeRow,
  makeSidebar,
  makeSnapshot,
  makeSubmission,
  makeSubmissionRequest,
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
const editSummary = vi.fn();
const requestSubmission = vi.fn();
const cancelSubmission = vi.fn();
const sendSubmission = vi.fn();

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
    editSummary: (body: unknown) => editSummary(body),
    requestSubmission: (event: unknown) => requestSubmission(event),
    cancelSubmission: () => cancelSubmission(),
    sendSubmission: (channel: unknown) => sendSubmission(channel),
  },
}));

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: () => Promise.resolve() }));

beforeEach(() => {
  describeWindow.mockReset();
  describeWindow.mockResolvedValue({
    Session: { session: { description: "pull request #42", row_identity: "acme/widgets#42" } },
  });
  openSession.mockReset();
  openSession.mockResolvedValue({
    status: "ok",
    data: makeSnapshot({
      sidebar: makeSidebar([makeFileSummary()]),
      can_submit: true,
      summary: "",
    }),
  });
  selectFile.mockReset();
  selectFile.mockResolvedValue({
    status: "ok",
    data: makeFile({
      rows: [makeRow({ text: "first" }), makeRow({ text: "second" })],
      drafts: makeDrafts({ ready_count: 2, not_anchored_count: 0 }),
    }),
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
  editSummary.mockReset();
  editSummary.mockResolvedValue({ status: "ok", data: null });
  requestSubmission.mockReset();
  cancelSubmission.mockReset();
  sendSubmission.mockReset();
});

/** Renders the Session and waits for the submit bar to appear. */
async function openSubmitBar() {
  render(<App />);
  await waitFor(() => expect(screen.getByRole("button", { name: "Approve" })).toBeTruthy());
}

/** The submit bar itself, so its Comment action is told from a diff row's own. */
function submitBar() {
  const bar = document.querySelector(".submit-bar");
  if (!bar) {
    throw new Error("the submit bar is not on screen");
  }
  return within(bar as HTMLElement);
}

/** The summary editor's CodeMirror content, once it has mounted. */
async function summaryField() {
  return await waitFor(() => {
    const field = document.querySelector("[data-summary-editor] .cm-content");
    if (!field) {
      throw new Error("the summary editor is not mounted yet");
    }
    return field as HTMLElement;
  });
}

/** Opens the confirmation for `event` and waits for it to be on screen. */
async function confirm(user: ReturnType<typeof userEvent.setup>, action: string) {
  await user.click(submitBar().getByRole("button", { name: action }));
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "Post this review to GitHub" })).toBeTruthy(),
  );
}

describe("the summary editor", () => {
  it("writes what is typed through on every keystroke", async () => {
    const user = userEvent.setup();
    await openSubmitBar();
    const field = await summaryField();

    await user.click(field);
    await user.keyboard("ok");

    await waitFor(() => expect(editSummary).toHaveBeenCalledWith("ok"));
  });

  it("restores what the snapshot brought back from storage", async () => {
    openSession.mockResolvedValue({
      status: "ok",
      data: makeSnapshot({
        sidebar: makeSidebar([makeFileSummary()]),
        can_submit: true,
        summary: "written last week",
      }),
    });
    await openSubmitBar();

    const field = await summaryField();
    expect(field.textContent).toContain("written last week");
    // Restoring is not typing, so nothing is written back.
    expect(editSummary).not.toHaveBeenCalled();
  });
});

describe("the confirmation", () => {
  it("shows the verdict, the summary, and every draft with its file and line", async () => {
    const user = userEvent.setup();
    requestSubmission.mockResolvedValue({
      status: "ok",
      data: makeSubmission({ state: "Confirming", request: makeSubmissionRequest() }),
    });
    await openSubmitBar();

    await confirm(user, "Comment");

    expect(screen.getByText("Comment with 1 inline comment")).toBeTruthy();
    expect(screen.getByText("pinned to abc1234")).toBeTruthy();
    expect(screen.getByText("Two notes.")).toBeTruthy();
    expect(screen.getByText("src/review_fixture_00.rs RIGHT line 2")).toBeTruthy();
    expect(screen.getByText("needs a test")).toBeTruthy();
    expect(requestSubmission).toHaveBeenCalledWith("Comment");
    expect(sendSubmission).not.toHaveBeenCalled();
  });

  it("names the drafts it would leave behind, with why", async () => {
    const user = userEvent.setup();
    requestSubmission.mockResolvedValue({
      status: "ok",
      data: makeSubmission({
        state: "Confirming",
        request: makeSubmissionRequest({
          excluded: [
            {
              position: "src/gone.rs line 12",
              reason: "not on a line in the current diff",
              body: "about the old head",
            },
          ],
          excluded_heading: "1 draft will NOT be posted",
        }),
      }),
    });
    await openSubmitBar();

    await confirm(user, "Approve");

    expect(screen.getByText("1 draft will NOT be posted")).toBeTruthy();
    expect(
      screen.getByText(
        "src/gone.rs line 12 (not on a line in the current diff): about the old head",
      ),
    ).toBeTruthy();
  });

  it("returns to the Session on Cancel, having posted nothing", async () => {
    const user = userEvent.setup();
    requestSubmission.mockResolvedValue({
      status: "ok",
      data: makeSubmission({ state: "Confirming", request: makeSubmissionRequest() }),
    });
    cancelSubmission.mockResolvedValue({ status: "ok", data: makeSubmission({ state: "Idle" }, 2) });
    await openSubmitBar();
    await confirm(user, "Comment");

    await user.click(screen.getByRole("button", { name: "Cancel" }));

    await waitFor(() =>
      expect(screen.queryByRole("button", { name: "Post this review to GitHub" })).toBeNull(),
    );
    expect(sendSubmission).not.toHaveBeenCalled();
    // The diff and the counts are exactly as they were.
    expect(screen.getByText("first")).toBeTruthy();
    expect(screen.getByText("2 to submit")).toBeTruthy();
  });

  it("shows what a review that cannot be assembled says, and posts nothing", async () => {
    const user = userEvent.setup();
    requestSubmission.mockResolvedValue({
      status: "ok",
      data: makeSubmission({
        state: "Failed",
        failure: {
          summary: "This review cannot be submitted yet",
          detail: null,
          remediation: "a comment review needs a summary",
        },
      }),
    });
    await openSubmitBar();

    await user.click(submitBar().getByRole("button", { name: "Comment" }));

    await waitFor(() =>
      expect(screen.getByText("This review cannot be submitted yet")).toBeTruthy(),
    );
    expect(screen.getByText("a comment review needs a summary")).toBeTruthy();
    expect(sendSubmission).not.toHaveBeenCalled();
  });
});

describe("sending the review", () => {
  it("reports what was posted and leaves no way to send it again", async () => {
    const user = userEvent.setup();
    requestSubmission.mockResolvedValue({
      status: "ok",
      data: makeSubmission({ state: "Confirming", request: makeSubmissionRequest() }),
    });
    sendSubmission.mockResolvedValue({
      status: "ok",
      data: {
        submission: makeSubmission(
          {
            state: "Sent",
            outcome: {
              heading: "Submitted as COMMENTED with 1 inline comment",
              url: "https://github.com/acme/widgets/pull/42",
            },
          },
          3,
        ),
        drafts: makeDrafts({ ready_count: 0, not_anchored_count: 0 }),
        summary: "",
      },
    });
    await openSubmitBar();
    await confirm(user, "Comment");

    await user.click(screen.getByRole("button", { name: "Post this review to GitHub" }));

    await waitFor(() =>
      expect(screen.getByText("Submitted as COMMENTED with 1 inline comment")).toBeTruthy(),
    );
    expect(screen.getByText("https://github.com/acme/widgets/pull/42")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Post this review to GitHub" })).toBeNull();
    expect(screen.getByText("0 to submit")).toBeTruthy();
    // The editor is emptied only once the forge has accepted it.
    const field = await summaryField();
    expect(field.textContent).toBe("");
  });

  it("keeps every draft and the summary when the send fails", async () => {
    const user = userEvent.setup();
    openSession.mockResolvedValue({
      status: "ok",
      data: makeSnapshot({
        sidebar: makeSidebar([makeFileSummary()]),
        can_submit: true,
        summary: "Two notes.",
      }),
    });
    requestSubmission.mockResolvedValue({
      status: "ok",
      data: makeSubmission({ state: "Confirming", request: makeSubmissionRequest() }),
    });
    sendSubmission.mockResolvedValue({
      status: "ok",
      data: {
        submission: makeSubmission(
          {
            state: "Failed",
            failure: {
              summary: "The pull request moved on",
              detail: "422 Unprocessable Entity",
              remediation: "Your drafts are unchanged.",
            },
          },
          3,
        ),
        drafts: makeDrafts({ ready_count: 2, not_anchored_count: 0 }),
        summary: "Two notes.",
      },
    });
    await openSubmitBar();
    await confirm(user, "Comment");

    await user.click(screen.getByRole("button", { name: "Post this review to GitHub" }));

    await waitFor(() => expect(screen.getByText("The pull request moved on")).toBeTruthy());
    expect(screen.getByText("Your drafts are unchanged.")).toBeTruthy();
    expect(screen.getByText("422 Unprocessable Entity")).toBeTruthy();
    expect(screen.getByText("2 to submit")).toBeTruthy();
    const field = await summaryField();
    expect(field.textContent).toContain("Two notes.");
  });

  it("says it is sending, and a second confirm sends nothing more", async () => {
    const user = userEvent.setup();
    requestSubmission.mockResolvedValue({
      status: "ok",
      data: makeSubmission({ state: "Confirming", request: makeSubmissionRequest() }),
    });
    let channel: Channel<SubmissionDto> | undefined;
    // Never settles, so the panel stays on the send it already started.
    sendSubmission.mockImplementation((given: Channel<SubmissionDto>) => {
      channel = given;
      return new Promise(() => {});
    });
    await openSubmitBar();
    await confirm(user, "Comment");

    const send = screen.getByRole("button", { name: "Post this review to GitHub" });
    await user.click(send);
    act(() => channel?.onmessage(makeSubmission({ state: "Sending" }, 2)));

    await waitFor(() => expect(screen.getByText("Submitting the review...")).toBeTruthy());
    // The confirmation is gone, so there is nothing left to press twice.
    expect(screen.queryByRole("button", { name: "Post this review to GitHub" })).toBeNull();
    await user.click(send);
    expect(sendSubmission).toHaveBeenCalledTimes(1);
  });
});

describe("a finding about the whole change", () => {
  it("puts its proposal in the summary editor and retires the finding", async () => {
    const user = userEvent.setup();
    reviewPanel.mockResolvedValue({
      status: "ok",
      data: makePanel({
        findings: [makeFinding({ id: 7, position: null, title: "no tests anywhere" })],
      }),
    });
    acceptFinding.mockResolvedValue({
      status: "ok",
      data: {
        panel: makePanel({ revision: 2, findings: [] }),
        drafts: makeDrafts({ ready_count: 2 }),
        disposition: { outcome: "Summary", body: "Add tests for the new branch." },
      },
    });
    await openSubmitBar();

    await user.click(screen.getByRole("button", { name: "Accept" }));

    expect(acceptFinding).toHaveBeenCalledWith(7);
    await waitFor(() => expect(screen.queryByText("no tests anywhere")).toBeNull());
    const field = await summaryField();
    expect(field.textContent).toContain("Add tests for the new branch.");
  });
});

describe("the submit bar", () => {
  it("shows the ready count and the three actions", async () => {
    await openSubmitBar();

    expect(screen.getByText("2 to submit")).toBeTruthy();
    expect(submitBar().getByRole("button", { name: "Comment" })).toBeTruthy();
    expect(submitBar().getByRole("button", { name: "Approve" })).toBeTruthy();
    expect(submitBar().getByRole("button", { name: "Request changes" })).toBeTruthy();
  });

  it("names the drafts that are no longer anchored, and only when there are some", async () => {
    selectFile.mockResolvedValue({
      status: "ok",
      data: makeFile({
        rows: [makeRow({ text: "first" })],
        drafts: makeDrafts({ ready_count: 1, not_anchored_count: 3 }),
      }),
    });
    await openSubmitBar();

    expect(screen.getByText("1 to submit")).toBeTruthy();
    expect(screen.getByText("3 not anchored")).toBeTruthy();
  });

  it("shows no not-anchored line when every draft still anchors", async () => {
    await openSubmitBar();

    expect(screen.queryByText(/not anchored/)).toBeNull();
  });

  it("offers no submit bar at all for a Session that cannot be submitted", async () => {
    openSession.mockResolvedValue({
      status: "ok",
      data: makeSnapshot({ sidebar: makeSidebar([makeFileSummary()]), can_submit: false }),
    });

    render(<App />);
    await waitFor(() => expect(screen.getByText("first")).toBeTruthy());

    expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
    expect(screen.queryByText(/to submit/)).toBeNull();
  });
});
