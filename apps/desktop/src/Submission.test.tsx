import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import App from "./App";
import {
  makeDrafts,
  makeFile,
  makeFileSummary,
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
    sendSubmission: () => sendSubmission(),
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
