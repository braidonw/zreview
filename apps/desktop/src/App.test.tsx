import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { Channel } from "@tauri-apps/api/core";
import App from "./App";
import { makeFile, makeFileSummary, makeRow, makeSidebar, makeSnapshot } from "./test/fixtures";

const describeLaunch = vi.fn();
const openSession = vi.fn();
const selectFile = vi.fn();
const toggleViewed = vi.fn();
const editDraft = vi.fn();
const discardDraft = vi.fn();
const reanchorDraft = vi.fn();

vi.mock("./bindings", () => ({
  commands: {
    describeLaunch: () => describeLaunch(),
    openSession: (channel: unknown) => openSession(channel),
    selectFile: (index: unknown) => selectFile(index),
    toggleViewed: () => toggleViewed(),
    editDraft: (...args: unknown[]) => editDraft(...args),
    discardDraft: (...args: unknown[]) => discardDraft(...args),
    reanchorDraft: (...args: unknown[]) => reanchorDraft(...args),
  },
}));

const writeText = vi.fn();
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  writeText: (text: unknown) => writeText(text),
}));

beforeEach(() => {
  describeLaunch.mockReset();
  describeLaunch.mockResolvedValue({ Session: { description: "the generated fixture" } });
  openSession.mockReset();
  selectFile.mockReset();
  toggleViewed.mockReset();
  editDraft.mockReset();
  discardDraft.mockReset();
  reanchorDraft.mockReset();
  writeText.mockReset();
});

describe("App", () => {
  it("shows the loading screen, updating as the channel reports stages", async () => {
    let channel: Channel<string> | undefined;
    openSession.mockImplementation((givenChannel: Channel<string>) => {
      channel = givenChannel;
      return new Promise(() => {});
    });

    render(<App />);

    await waitFor(() => expect(screen.getByText(/Opening the generated fixture/)).toBeTruthy());

    channel?.onmessage("Fetching Git objects");

    await waitFor(() => expect(screen.getByText("Fetching Git objects")).toBeTruthy());
  });

  it("shows twelve sidebar rows once the session and first file load", async () => {
    const files = Array.from({ length: 12 }, (_, index) =>
      makeFileSummary({
        index,
        path: `src/review_fixture_${String(index).padStart(2, "0")}.rs`,
      }),
    );
    openSession.mockResolvedValue({
      status: "ok",
      data: makeSnapshot({ sidebar: makeSidebar(files) }),
    });
    selectFile.mockResolvedValue({ status: "ok", data: makeFile() });

    render(<App />);

    await waitFor(() =>
      expect(screen.getAllByText(/review_fixture_/)).toHaveLength(12),
    );
  });

  it("shows the failure screen's fields when opening fails", async () => {
    openSession.mockResolvedValue({
      status: "error",
      error: { summary: "Could not load", detail: "boom", remediation: "try again" },
    });

    render(<App />);

    await waitFor(() => expect(screen.getByText("Could not load")).toBeTruthy());
    expect(screen.getByText("boom")).toBeTruthy();
    expect(screen.getByText("try again")).toBeTruthy();
  });

  it("renders the failure screen for a plain-string command rejection", async () => {
    openSession.mockResolvedValue({ status: "error", error: "network unreachable" });

    render(<App />);

    await waitFor(() => expect(screen.getByText("network unreachable")).toBeTruthy());
  });

  it("shows the failure screen when openSession itself rejects", async () => {
    openSession.mockRejectedValue(new Error("IPC is unavailable"));

    render(<App />);

    await waitFor(() => expect(screen.getByText("Error: IPC is unavailable")).toBeTruthy());
  });

  describe("once ready", () => {
    beforeEach(() => {
      const files = [makeFileSummary({ index: 0 }), makeFileSummary({ index: 1 })];
      openSession.mockResolvedValue({
        status: "ok",
        data: makeSnapshot({ sidebar: makeSidebar(files) }),
      });
      selectFile.mockResolvedValue({
        status: "ok",
        data: makeFile({
          rows: [
            makeRow({ text: "first" }),
            makeRow({ text: "second" }),
            makeRow({ text: "third" }),
          ],
        }),
      });
      toggleViewed.mockResolvedValue({ status: "ok", data: makeSidebar(files) });
    });

    it("selects the next file on cmd-shift-J", async () => {
      const user = userEvent.setup();
      render(<App />);
      await waitFor(() => expect(screen.getByText("first")).toBeTruthy());
      selectFile.mockClear();

      await user.keyboard("{Meta>}{Shift>}j{/Shift}{/Meta}");

      expect(selectFile).toHaveBeenCalledWith(1);
    });

    it("updates the viewed count on cmd-shift-V", async () => {
      const user = userEvent.setup();
      const updatedFiles = [
        makeFileSummary({ index: 0, viewed: true }),
        makeFileSummary({ index: 1 }),
      ];
      toggleViewed.mockResolvedValue({ status: "ok", data: makeSidebar(updatedFiles) });

      render(<App />);
      await waitFor(() => expect(screen.getByText("first")).toBeTruthy());

      await user.keyboard("{Meta>}{Shift>}v{/Shift}{/Meta}");

      await waitFor(() => expect(screen.getByText(/1 viewed/)).toBeTruthy());
    });

    it("copies the joined selected rows' text on cmd-C", async () => {
      const user = userEvent.setup();
      render(<App />);
      await waitFor(() => expect(screen.getByText("first")).toBeTruthy());

      await user.keyboard("{Meta>}c{/Meta}");

      expect(writeText).toHaveBeenCalledWith("first");
    });

    it("moves the cursor down with j and up with k", async () => {
      const user = userEvent.setup();
      render(<App />);
      await waitFor(() => expect(screen.getByText("first")).toBeTruthy());

      await user.keyboard("j");
      expect(screen.getByText("second").closest(".diff-row")?.className).toContain(
        "diff-row--selected",
      );

      await user.keyboard("k");
      expect(screen.getByText("first").closest(".diff-row")?.className).toContain(
        "diff-row--selected",
      );
    });

    it("extends the selection with shift+j without moving the anchor", async () => {
      const user = userEvent.setup();
      render(<App />);
      await waitFor(() => expect(screen.getByText("first")).toBeTruthy());

      await user.keyboard("{Shift>}j{/Shift}");

      expect(screen.getByText("first").closest(".diff-row")?.className).toContain(
        "diff-row--in-selection",
      );
      expect(screen.getByText("second").closest(".diff-row")?.className).toContain(
        "diff-row--in-selection",
      );
    });

    it("copies the joined text of a multi-row selection on cmd-C", async () => {
      const user = userEvent.setup();
      render(<App />);
      await waitFor(() => expect(screen.getByText("first")).toBeTruthy());

      await user.keyboard("{Shift>}j{/Shift}");
      await user.keyboard("{Meta>}c{/Meta}");

      expect(writeText).toHaveBeenCalledWith("first\nsecond");
    });

    it("yields the row cursor to a keydown targeting the open composer", async () => {
      const user = userEvent.setup();
      render(<App />);
      await waitFor(() => expect(screen.getByText("first")).toBeTruthy());

      await user.keyboard("c");
      const editorContent = await waitFor(() => {
        const element = document.querySelector("[data-composer] .cm-content");
        if (!element) {
          throw new Error("composer editor not mounted yet");
        }
        return element;
      });

      editorContent.dispatchEvent(new KeyboardEvent("keydown", { key: "j", bubbles: true }));

      expect(screen.getByText("first").closest(".diff-row")?.className).toContain(
        "diff-row--selected",
      );
      expect(screen.getByText("second").closest(".diff-row")?.className).not.toContain(
        "diff-row--selected",
      );
    });

    it("does not let the global c toggle swallow a keydown inside the composer", async () => {
      const user = userEvent.setup();
      render(<App />);
      await waitFor(() => expect(screen.getByText("first")).toBeTruthy());

      await user.keyboard("c");
      const editorContent = await waitFor(() => {
        const element = document.querySelector("[data-composer] .cm-content");
        if (!element) {
          throw new Error("composer editor not mounted yet");
        }
        return element;
      });

      editorContent.dispatchEvent(new KeyboardEvent("keydown", { key: "c", bubbles: true }));

      expect(document.querySelector("[data-composer]")).not.toBeNull();
    });
  });
});
