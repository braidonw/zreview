import { readFileSync } from "node:fs";
import { join } from "node:path";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "./App";
import {
  makeFile,
  makeFileSummary,
  makeRow,
  makeSidebar,
  makeSnapshot,
  makeSubmission,
  makeSubmissionRequest,
} from "./test/fixtures";

const describeWindow = vi.fn();
const openSession = vi.fn();
const selectFile = vi.fn();

vi.mock("./bindings", () => ({
  commands: {
    describeWindow: () => describeWindow(),
    openSession: (channel: unknown) => openSession(channel),
    selectFile: (index: unknown) => selectFile(index),
    toggleViewed: () => Promise.resolve({ status: "ok", data: null }),
    editDraft: () => Promise.resolve({ status: "ok", data: null }),
    discardDraft: () => Promise.resolve({ status: "ok", data: null }),
    reanchorDraft: () => Promise.resolve({ status: "ok", data: null }),
    reviewPanel: () => Promise.resolve({ status: "ok", data: null }),
    editSummary: () => Promise.resolve({ status: "ok", data: null }),
    requestSubmission: () =>
      Promise.resolve({
        status: "ok",
        data: makeSubmission({ state: "Confirming", request: makeSubmissionRequest() }),
      }),
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
    data: makeSnapshot({ sidebar: makeSidebar([makeFileSummary()]), can_submit: true }),
  });
  selectFile.mockReset();
  selectFile.mockResolvedValue({
    status: "ok",
    data: makeFile({ rows: [makeRow({ text: "first" })] }),
  });
});

// jsdom does not load imported stylesheets, so getComputedStyle can't see index.css.
// It does implement the real :focus-visible heuristic (keyboard focus vs. mouse
// focus) independent of any stylesheet, so that part is asserted behaviourally.
describe("focus ring", () => {
  it("defines a :focus-visible rule built on --border-focus in index.css", () => {
    const css = readFileSync(join(process.cwd(), "src/index.css"), "utf-8");
    expect(css).toMatch(/:focus-visible\s*{[^}]*--border-focus/);
  });

  it("shows a keyboard-focused submit bar button as :focus-visible, not a mouse-focused one", async () => {
    const user = userEvent.setup();
    render(<App />);
    const approveButton = await screen.findByRole("button", { name: "Approve" });

    for (let i = 0; i < 60 && document.activeElement !== approveButton; i++) {
      await user.tab();
    }
    expect(document.activeElement).toBe(approveButton);
    expect(approveButton.matches(":focus-visible")).toBe(true);

    await user.click(approveButton);
    expect(approveButton.matches(":focus-visible")).toBe(false);
  });

  it("rings the summary editor's CodeMirror container on focus, built on --border-focus", async () => {
    const user = userEvent.setup();
    render(<App />);
    await screen.findByRole("button", { name: "Approve" });

    const editorContent = document.querySelector(".summary-editor__field .cm-content");
    if (!editorContent) {
      throw new Error("summary editor did not mount");
    }
    await user.click(editorContent);

    const container = editorContent.closest(".cm-editor");
    expect(container?.classList.contains("cm-focused")).toBe(true);

    const css = readFileSync(join(process.cwd(), "src/components/SummaryEditor.css"), "utf-8");
    expect(css).toMatch(/cm-focused[^{]*{[^}]*--border-focus/);
  });
});
