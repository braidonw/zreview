import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { HomeSnapshotDto, OpenSessionDto, WindowDto } from "./bindings";
import App from "./App";
import {
  makeAnchoredDraft,
  makeDrafts,
  makeFile,
  makeFileSummary,
  makeHomeGroups,
  makeHomeRepository,
  makeHomeRow,
  makeHomeSnapshot,
  makeRow,
  makeSidebar,
  makeSnapshot,
} from "./test/fixtures";

const describeWindow = vi.fn();
const openRow = vi.fn();
const returnToHome = vi.fn();
const returnToSession = vi.fn();
const refreshHome = vi.fn();
const moveHomeCursor = vi.fn();
const toggleRepositoriesFooter = vi.fn();
const addRepositories = vi.fn();
const removeRepository = vi.fn();
const openSession = vi.fn();
const selectFile = vi.fn();
const toggleViewed = vi.fn();
const editDraft = vi.fn();
const discardDraft = vi.fn();
const reanchorDraft = vi.fn();

vi.mock("./bindings", () => ({
  commands: {
    describeWindow: () => describeWindow(),
    openRow: (repository: unknown, number: unknown) => openRow(repository, number),
    returnToHome: () => returnToHome(),
    returnToSession: () => returnToSession(),
    refreshHome: (onProgress: unknown) => refreshHome(onProgress),
    moveHomeCursor: (moveTo: unknown) => moveHomeCursor(moveTo),
    toggleRepositoriesFooter: () => toggleRepositoriesFooter(),
    addRepositories: (folders: unknown) => addRepositories(folders),
    removeRepository: (path: unknown) => removeRepository(path),
    openSession: (channel: unknown) => openSession(channel),
    selectFile: (index: unknown) => selectFile(index),
    toggleViewed: () => toggleViewed(),
    editDraft: (...args: unknown[]) => editDraft(...args),
    discardDraft: (...args: unknown[]) => discardDraft(...args),
    reanchorDraft: (...args: unknown[]) => reanchorDraft(...args),
  },
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: () => Promise.resolve(null) }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: () => Promise.resolve() }));

/** The handler the screen gave Tauri's window focus event, once it is listening. */
let focusChanged: ((event: { payload: boolean }) => void) | null = null;
const stopListening = vi.fn();
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onFocusChanged: (handler: (event: { payload: boolean }) => void) => {
      focusChanged = handler;
      return Promise.resolve(stopListening);
    },
  }),
}));

function ok<T>(data: T) {
  return { status: "ok", data };
}

/** The Session opened from `acme/widgets#412`, as the window describes it. */
function alive(identity = "acme/widgets#412"): OpenSessionDto {
  return { description: "pull request #412", row_identity: identity };
}

function showingHome(session: OpenSessionDto | null = null): WindowDto {
  return { Home: { alive: session } };
}

function showingSession(session: OpenSessionDto = alive()): WindowDto {
  return { Session: { session } };
}

/** Two rows to review, one of which the alive Session was opened from. */
function listed(): HomeSnapshotDto {
  return makeHomeSnapshot({
    count_line: "2 pull requests across 1 repository",
    groups: makeHomeGroups([
      [
        makeHomeRow({
          index: 0,
          repository: "acme/widgets",
          number: 412,
          identity: "acme/widgets#412",
          title: "Retry webhook deliveries",
        }),
        makeHomeRow({
          index: 1,
          repository: "acme/widgets",
          number: 398,
          identity: "acme/widgets#398",
          title: "Split the invoice renderer",
        }),
      ],
      [],
      [],
    ]),
    repositories: [makeHomeRepository()],
    footer_summary: "1 repository",
    refresh: { Refreshed: { at_ms: Date.now() } },
  });
}

/** Every row on screen, in the order the ledger renders them. */
function rows() {
  return screen.getAllByRole("listitem");
}

/** The row whose identity cell reads `identity`. */
function rowFor(identity: string) {
  const found = rows().find(
    (row) => row.querySelector(".home__cell--identity")?.textContent === identity,
  );
  if (!found) {
    throw new Error(`no row for ${identity}`);
  }
  return found;
}

beforeEach(() => {
  focusChanged = null;
  stopListening.mockReset();
  describeWindow.mockReset();
  describeWindow.mockResolvedValue(showingHome());
  openRow.mockReset();
  openRow.mockResolvedValue(ok(showingSession()));
  returnToHome.mockReset();
  returnToHome.mockResolvedValue(ok(showingHome(alive())));
  returnToSession.mockReset();
  returnToSession.mockResolvedValue(ok(showingSession()));
  refreshHome.mockReset();
  refreshHome.mockResolvedValue(ok(listed()));
  moveHomeCursor.mockReset();
  moveHomeCursor.mockResolvedValue({ ...listed(), cursor: 1 });
  toggleRepositoriesFooter.mockReset();
  addRepositories.mockReset();
  removeRepository.mockReset();
  openSession.mockReset();
  openSession.mockResolvedValue(
    ok(makeSnapshot({ sidebar: makeSidebar([makeFileSummary({ index: 0 }), makeFileSummary({ index: 1 })]) })),
  );
  selectFile.mockReset();
  selectFile.mockResolvedValue(
    ok(makeFile({ rows: [makeRow({ text: "first" }), makeRow({ text: "second" })] })),
  );
  toggleViewed.mockReset();
  editDraft.mockReset();
  discardDraft.mockReset();
  reanchorDraft.mockReset();
});

/** Home, listed and settled, with the cursor on its first row. */
async function openHome() {
  render(<App />);
  await waitFor(() => expect(screen.getByText("Retry webhook deliveries")).toBeTruthy());
}

/** The Session the cursor row opens into, loaded and showing its first file. */
async function openTheCursorRow() {
  await openHome();
  fireEvent.keyDown(window, { key: "Enter" });
  await waitFor(() => expect(screen.getByText("first")).toBeTruthy());
}

describe("opening a row", () => {
  it("opens the row under the cursor on Enter", async () => {
    await openTheCursorRow();

    expect(openRow).toHaveBeenCalledWith("acme/widgets", 412);
    expect(screen.queryByText("Retry webhook deliveries")).toBeNull();
  });

  it("opens the row a click lands on", async () => {
    const user = userEvent.setup();
    await openHome();

    await user.click(screen.getByText("Split the invoice renderer"));

    await waitFor(() => expect(openRow).toHaveBeenCalledWith("acme/widgets", 398));
  });

  it("shows the failure when the row could not be opened", async () => {
    openRow.mockResolvedValue({
      status: "error",
      error: {
        summary: "Home has no configured clone of acme/widgets",
        detail: null,
        remediation: null,
      },
    });
    await openHome();

    fireEvent.keyDown(window, { key: "Enter" });

    await waitFor(() =>
      expect(screen.getByText("Home has no configured clone of acme/widgets")).toBeTruthy(),
    );
  });
});

describe("coming back", () => {
  it("returns to Home on the back chevron and refreshes it", async () => {
    const user = userEvent.setup();
    await openTheCursorRow();
    refreshHome.mockClear();

    await user.click(screen.getByRole("button", { name: "Back to Home" }));

    await waitFor(() => expect(screen.getByText("Retry webhook deliveries")).toBeTruthy());
    expect(refreshHome).toHaveBeenCalled();
  });

  it("returns to Home on cmd-[", async () => {
    await openTheCursorRow();

    fireEvent.keyDown(window, { key: "[", metaKey: true });

    await waitFor(() => expect(returnToHome).toHaveBeenCalled());
  });

  it("leaves the Session showing on Escape", async () => {
    await openTheCursorRow();

    fireEvent.keyDown(window, { key: "Escape" });

    expect(returnToHome).not.toHaveBeenCalled();
    expect(screen.getByText("first")).toBeTruthy();
  });

  it("offers neither chevron nor cmd-[ to a Session the command line opened", async () => {
    describeWindow.mockResolvedValue(
      showingSession({ description: "the generated fixture", row_identity: null }),
    );

    render(<App />);
    await waitFor(() => expect(screen.getByText("first")).toBeTruthy());

    expect(screen.queryByRole("button", { name: "Back to Home" })).toBeNull();
    fireEvent.keyDown(window, { key: "[", metaKey: true });
    expect(returnToHome).not.toHaveBeenCalled();
  });
});

describe("the Session kept alive behind Home", () => {
  it("keeps its tree mounted and hidden, with its file, cursor, and composer intact", async () => {
    const user = userEvent.setup();
    selectFile.mockResolvedValue(
      ok(
        makeFile({
          index: 1,
          rows: [makeRow({ text: "first" }), makeRow({ text: "second" })],
          drafts: makeDrafts({
            file_index: 1,
            anchored: [makeAnchoredDraft({ row: 1, body: "worth keeping" })],
          }),
        }),
      ),
    );
    await openTheCursorRow();
    await user.keyboard("j");
    await user.keyboard("c");
    const composer = await waitFor(() => {
      const element = document.querySelector("[data-composer] .cm-content");
      if (!element) {
        throw new Error("composer editor not mounted yet");
      }
      return element;
    });
    expect(composer.textContent).toContain("worth keeping");

    fireEvent.keyDown(window, { key: "[", metaKey: true });
    await waitFor(() => expect(screen.getByText("Retry webhook deliveries")).toBeTruthy());

    // Hidden rather than unmounted, which is what makes returning free.
    expect(document.querySelector(".app__session")?.hasAttribute("hidden")).toBe(true);
    expect(document.querySelector("[data-composer] .cm-content")).toBe(composer);
    await user.click(screen.getByRole("button", { name: /acme\/widgets#412/ }));

    await waitFor(() => expect(screen.getByText("first")).toBeTruthy());
    expect(document.querySelector("[data-composer] .cm-content")).toBe(composer);
    expect(composer.textContent).toContain("worth keeping");
    expect(screen.getByText("second").closest(".diff-row")?.className).toContain(
      "diff-row--selected",
    );
  });

  it("leaves the hidden Session's keys alone while Home has them", async () => {
    const user = userEvent.setup();
    await openTheCursorRow();
    fireEvent.keyDown(window, { key: "[", metaKey: true });
    await waitFor(() => expect(screen.getByText("Retry webhook deliveries")).toBeTruthy());

    await user.keyboard("j");

    expect(moveHomeCursor).toHaveBeenCalledWith("Down");
    expect(screen.getByText("first").closest(".diff-row")?.className).toContain(
      "diff-row--selected",
    );
  });

  it("shows the header slot with the alive Session's identity and its keycap", async () => {
    describeWindow.mockResolvedValue(showingHome(alive()));

    await openHome();

    const slot = screen.getByRole("button", { name: /acme\/widgets#412/ });
    expect(slot.textContent).toContain("acme/widgets#412");
    expect(slot.textContent).toContain("cmd-[");
  });

  it("shows no header slot when no Session is alive", async () => {
    await openHome();

    expect(screen.queryByText("acme/widgets#412", { selector: ".home__slot-identity" })).toBeNull();
  });

  it("marks the alive Session's row and leaves every other row unmarked", async () => {
    describeWindow.mockResolvedValue(showingHome(alive()));

    await openHome();

    expect(rowFor("acme/widgets#412").querySelector(".home__alive-mark")).toBeTruthy();
    expect(rowFor("acme/widgets#398").querySelector(".home__alive-mark")).toBeNull();
  });

  it("returns to the alive Session rather than reloading it when its row is opened", async () => {
    describeWindow.mockResolvedValue(showingHome(alive()));
    await openHome();

    fireEvent.keyDown(window, { key: "Enter" });

    await waitFor(() => expect(returnToSession).toHaveBeenCalled());
    expect(openRow).not.toHaveBeenCalled();
  });

  it("still opens a different row while a Session is alive", async () => {
    const user = userEvent.setup();
    describeWindow.mockResolvedValue(showingHome(alive()));
    await openHome();

    await user.click(screen.getByText("Split the invoice renderer"));

    await waitFor(() => expect(openRow).toHaveBeenCalledWith("acme/widgets", 398));
    expect(returnToSession).not.toHaveBeenCalled();
  });

  it("reaches a Session whose pull request has no row through the slot alone", async () => {
    describeWindow.mockResolvedValue(showingHome(alive("acme/billing#7")));
    const user = userEvent.setup();
    await openHome();

    expect(rows().some((row) => row.querySelector(".home__alive-mark"))).toBe(false);
    await user.click(screen.getByRole("button", { name: /acme\/billing#7/ }));

    await waitFor(() => expect(returnToSession).toHaveBeenCalled());
  });
});

describe("refreshing on focus", () => {
  it("refreshes Home when the window regains focus", async () => {
    await openHome();
    refreshHome.mockClear();

    focusChanged?.({ payload: true });

    await waitFor(() => expect(refreshHome).toHaveBeenCalledTimes(1));
  });

  it("refreshes nothing when the window loses focus", async () => {
    await openHome();
    refreshHome.mockClear();

    focusChanged?.({ payload: false });

    expect(refreshHome).not.toHaveBeenCalled();
  });

  it("stops listening once the Session replaces Home", async () => {
    await openTheCursorRow();

    expect(stopListening).toHaveBeenCalled();
  });

  it("refreshes on no timer at all", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      await openHome();
      refreshHome.mockClear();

      await vi.advanceTimersByTimeAsync(10 * 60_000);

      expect(refreshHome).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });
});
