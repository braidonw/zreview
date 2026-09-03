import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { HomeSnapshotDto } from "./bindings";
import App from "./App";
import {
  makeHomeGroups,
  makeHomeRepository,
  makeHomeRow,
  makeHomeSnapshot,
} from "./test/fixtures";

const describeLaunch = vi.fn();
const refreshHome = vi.fn();
const addRepositories = vi.fn();
const removeRepository = vi.fn();
const toggleRepositoriesFooter = vi.fn();

const moveHomeCursor = vi.fn();

vi.mock("./bindings", () => ({
  commands: {
    describeLaunch: () => describeLaunch(),
    refreshHome: (onProgress: unknown) => refreshHome(onProgress),
    moveHomeCursor: (moveTo: unknown) => moveHomeCursor(moveTo),
    addRepositories: (folders: unknown) => addRepositories(folders),
    removeRepository: (path: unknown) => removeRepository(path),
    toggleRepositoriesFooter: () => toggleRepositoriesFooter(),
  },
}));

const open = vi.fn();
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (options: unknown) => open(options),
}));

// The commands that touch the settings file answer with a result, as the bindings do.
function ok(snapshot: HomeSnapshotDto) {
  return { status: "ok", data: snapshot };
}

// Home with two clones configured, one of which did not resolve.
function configured() {
  return makeHomeSnapshot({
    count_line: "0 pull requests across 2 repositories",
    repositories: [
      makeHomeRepository({ path: "/Developer/zreview", slug: "braidonw/zreview" }),
      makeHomeRepository({
        path: "/Developer/moved",
        slug: null,
        failure: "the folder no longer exists",
      }),
    ],
    footer_summary: "2 repositories · 1 failed",
  });
}

beforeEach(() => {
  describeLaunch.mockReset();
  describeLaunch.mockResolvedValue("Home");
  refreshHome.mockReset();
  refreshHome.mockResolvedValue(ok(makeHomeSnapshot()));
  addRepositories.mockReset();
  removeRepository.mockReset();
  toggleRepositoriesFooter.mockReset();
  moveHomeCursor.mockReset();
  open.mockReset();
});

describe("Home", () => {
  it("renders the heading, the count line, and the three groups with their empty copy", async () => {
    refreshHome.mockResolvedValue(ok(configured()));

    render(<App />);

    await waitFor(() => expect(screen.getByText("Home")).toBeTruthy());
    expect(screen.getByText("0 pull requests across 2 repositories")).toBeTruthy();
    expect(screen.getByText("To review")).toBeTruthy();
    expect(screen.getByText("To address")).toBeTruthy();
    expect(screen.getByText("Waiting on others")).toBeTruthy();
    expect(screen.getByText("Nothing waiting for your review.")).toBeTruthy();
    expect(screen.getByText("Nothing to address.")).toBeTruthy();
    expect(screen.getByText("Nothing waiting on others.")).toBeTruthy();
  });

  it("shows the footer collapsed to its summary line with Add beside it", async () => {
    refreshHome.mockResolvedValue(ok(configured()));

    render(<App />);

    await waitFor(() => expect(screen.getByText("Repositories")).toBeTruthy());
    expect(screen.getByText("2 repositories · 1 failed")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Add..." })).toBeTruthy();
    expect(screen.queryByText("/Developer/zreview")).toBeNull();
  });

  it("lists each repository's slug, path, state, and Remove once the footer expands", async () => {
    const user = userEvent.setup();
    refreshHome.mockResolvedValue(ok(configured()));
    toggleRepositoriesFooter.mockResolvedValue({ ...configured(), footer_expanded: true });

    render(<App />);
    await waitFor(() => expect(screen.getByText("Repositories")).toBeTruthy());

    await user.click(screen.getByRole("button", { name: /Repositories/ }));

    await waitFor(() => expect(screen.getByText("braidonw/zreview")).toBeTruthy());
    expect(screen.getByText("/Developer/zreview")).toBeTruthy();
    expect(screen.getByText("the folder no longer exists")).toBeTruthy();
    expect(screen.getAllByRole("button", { name: "Remove" })).toHaveLength(2);
  });

  it("shows the centred empty state and the no repositories summary on first launch", async () => {
    render(<App />);

    await waitFor(() => expect(screen.getByText("No repositories yet")).toBeTruthy());
    expect(screen.getByText("No repositories")).toBeTruthy();
    expect(screen.queryByText("To review")).toBeNull();
    expect(screen.getAllByRole("button", { name: /Add/ }).length).toBeGreaterThan(0);
  });

  it("replaces the list with the failure block while leaving Add in the footer", async () => {
    refreshHome.mockResolvedValue(
      ok(
        makeHomeSnapshot({
          failure: {
            summary: "Home could not read your settings",
            detail: "could not parse the settings file",
            remediation: "Fix ~/.config/zreview/settings.toml, then press r to refresh.",
          },
        }),
      ),
    );

    render(<App />);

    await waitFor(() =>
      expect(screen.getByText("Home could not read your settings")).toBeTruthy(),
    );
    expect(screen.getByText("could not parse the settings file")).toBeTruthy();
    expect(
      screen.getByText("Fix ~/.config/zreview/settings.toml, then press r to refresh."),
    ).toBeTruthy();
    expect(screen.queryByText("To review")).toBeNull();
    expect(screen.getByRole("button", { name: "Add..." })).toBeTruthy();
  });

  it("shows a write failure as a line above a list that stays", async () => {
    refreshHome.mockResolvedValue(
      ok({
        ...configured(),
        write_failure: {
          summary: "Home could not save your settings",
          detail: null,
          remediation: "Check that ~/.config/zreview/settings.toml is writable.",
        },
      }),
    );

    render(<App />);

    await waitFor(() =>
      expect(screen.getByText("Home could not save your settings")).toBeTruthy(),
    );
    expect(
      screen.getByText("Check that ~/.config/zreview/settings.toml is writable."),
    ).toBeTruthy();
    expect(screen.getByText("To review")).toBeTruthy();
  });

  it("passes the picked folders to the add command and shows what it refused", async () => {
    const user = userEvent.setup();
    open.mockResolvedValue(["/Developer/notes", "/Developer/billing"]);
    addRepositories.mockResolvedValue(
      ok(
        makeHomeSnapshot({
          count_line: "0 pull requests across 1 repository",
          repositories: [makeHomeRepository({ path: "/Developer/billing", slug: "acme/billing" })],
          footer_summary: "1 repository",
          refusals: [{ path: "/Developer/notes", reason: "not a Git repository" }],
        }),
      ),
    );

    render(<App />);
    await waitFor(() => expect(screen.getByText("No repositories yet")).toBeTruthy());

    await user.click(screen.getByRole("button", { name: "Add repository..." }));

    await waitFor(() =>
      expect(addRepositories).toHaveBeenCalledWith(["/Developer/notes", "/Developer/billing"]),
    );
    expect(open).toHaveBeenCalledWith(
      expect.objectContaining({ directory: true, multiple: true }),
    );
    await waitFor(() =>
      expect(screen.getByText("/Developer/notes: not a Git repository")).toBeTruthy(),
    );
    expect(screen.getByText("1 repository")).toBeTruthy();
  });

  it("adds nothing when the picker is dismissed", async () => {
    const user = userEvent.setup();
    open.mockResolvedValue(null);

    render(<App />);
    await waitFor(() => expect(screen.getByText("No repositories yet")).toBeTruthy());

    await user.click(screen.getByRole("button", { name: "Add repository..." }));

    await waitFor(() => expect(open).toHaveBeenCalled());
    expect(addRepositories).not.toHaveBeenCalled();
  });

  it("removes the repository the Remove beside it names", async () => {
    const user = userEvent.setup();
    refreshHome.mockResolvedValue(ok({ ...configured(), footer_expanded: true }));
    removeRepository.mockImplementation((path: string) =>
      Promise.resolve(
        ok({
          ...configured(),
          footer_expanded: true,
          repositories: configured().repositories.filter(
            (repository) => repository.path !== path,
          ),
        }),
      ),
    );

    render(<App />);
    await waitFor(() => expect(screen.getByText("braidonw/zreview")).toBeTruthy());

    await user.click(screen.getAllByRole("button", { name: "Remove" })[0]);

    await waitFor(() => expect(screen.queryByText("/Developer/zreview")).toBeNull());
    expect(screen.getByText("/Developer/moved")).toBeTruthy();
  });

  // Three rows across the three groups, which is what the cursor walks.
  function listed() {
    return makeHomeSnapshot({
      count_line: "3 pull requests across 2 repositories",
      groups: makeHomeGroups([
        [
          makeHomeRow({
            index: 0,
            identity: "acme/widgets#412",
            title: "Retry webhook deliveries",
            check_status: { label: "checks passing", tone: "Success" },
          }),
          makeHomeRow({
            index: 1,
            identity: "acme/widgets#398",
            title: "Split the invoice renderer",
            author: "priya",
            review_status: { label: "you reviewed this head", tone: "Muted" },
            check_status: { label: "checks failing", tone: "Error" },
          }),
        ],
        [
          makeHomeRow({
            index: 2,
            identity: "braidonw/zreview#77",
            title: "Bound the retry ceiling",
            author: "braidonw",
            review_status: { label: "changes requested", tone: "Error" },
            check_status: { label: "checks running", tone: "Warning" },
          }),
        ],
        [],
      ]),
      repositories: [makeHomeRepository()],
      footer_summary: "2 repositories",
      // Two and a half minutes back, so a second either way still reads as 2 min.
      refresh: { Refreshed: { at_ms: Date.now() - 150_000 } },
    });
  }

  it("renders every row in group order with its statuses and its age", async () => {
    refreshHome.mockResolvedValue(ok(listed()));

    render(<App />);

    await waitFor(() => expect(screen.getByText("Retry webhook deliveries")).toBeTruthy());
    const rows = screen.getAllByRole("listitem");
    expect(rows.map((row) => row.getAttribute("data-identity"))).toEqual([
      "acme/widgets#412",
      "acme/widgets#398",
      "braidonw/zreview#77",
    ]);
    expect(screen.getByText("you reviewed this head")).toBeTruthy();
    expect(screen.getByText("changes requested")).toBeTruthy();
    expect(screen.getByText("checks passing")).toBeTruthy();
    expect(screen.getByText("checks failing")).toBeTruthy();
    expect(screen.getByText("checks running")).toBeTruthy();
    expect(screen.getByText("Nothing waiting on others.")).toBeTruthy();
  });

  it("leaves an aligned gap where a row has no status", async () => {
    refreshHome.mockResolvedValue(ok(listed()));

    render(<App />);

    await waitFor(() => expect(screen.getByText("Retry webhook deliveries")).toBeTruthy());
    // Every row carries the same columns whether or not they say anything.
    for (const row of screen.getAllByRole("listitem")) {
      expect(row.querySelectorAll(".home__cell").length).toBe(6);
    }
  });

  it("shows the stamp counting off the repositories while a refresh runs", async () => {
    refreshHome.mockResolvedValue(
      ok(makeHomeSnapshot({ refresh: { Refreshing: { done: 2, total: 4 } } })),
    );

    render(<App />);

    await waitFor(() => expect(screen.getByText("Refreshing 2 of 4")).toBeTruthy());
  });

  it("shows the stamp as how long ago the refresh settled", async () => {
    refreshHome.mockResolvedValue(ok(listed()));

    render(<App />);

    await waitFor(() => expect(screen.getByText("Refreshed 2 min ago")).toBeTruthy());
    expect(screen.getByText("r")).toBeTruthy();
  });

  it("shows the stamp as a failure when nothing loaded", async () => {
    refreshHome.mockResolvedValue(ok(makeHomeSnapshot({ refresh: "Failed" })));

    render(<App />);

    await waitFor(() => expect(screen.getByText("Refresh failed")).toBeTruthy());
  });

  it("shows no stamp at all before the first refresh answers", async () => {
    refreshHome.mockResolvedValue(ok(makeHomeSnapshot({ refresh: "NeverRefreshed" })));

    render(<App />);

    await waitFor(() => expect(screen.getByText("No repositories yet")).toBeTruthy());
    expect(screen.queryByText(/^Refresh/)).toBeNull();
  });

  it("moves the cursor down on j and up on k", async () => {
    const user = userEvent.setup();
    refreshHome.mockResolvedValue(ok(listed()));
    moveHomeCursor.mockImplementation((moveTo: string) =>
      Promise.resolve({ ...listed(), cursor: moveTo === "Down" ? 1 : 0 }),
    );

    render(<App />);
    await waitFor(() => expect(screen.getByText("Retry webhook deliveries")).toBeTruthy());

    await user.keyboard("j");

    await waitFor(() => expect(moveHomeCursor).toHaveBeenCalledWith("Down"));
    await waitFor(() =>
      expect(
        screen.getAllByRole("listitem")[1].getAttribute("data-cursor"),
      ).toBe("true"),
    );

    await user.keyboard("k");

    await waitFor(() => expect(moveHomeCursor).toHaveBeenCalledWith("Up"));
    await waitFor(() =>
      expect(
        screen.getAllByRole("listitem")[0].getAttribute("data-cursor"),
      ).toBe("true"),
    );
  });

  it("shows a failed repository above the list with a Remove beside it", async () => {
    refreshHome.mockResolvedValue(
      ok({
        ...listed(),
        failed_repositories: [
          {
            path: "/Developer/billing",
            slug: "acme/billing",
            reason: "GitHub refused the request: SAML enforcement",
          },
        ],
      }),
    );

    render(<App />);

    await waitFor(() =>
      expect(
        screen.getByText(/acme\/billing.*SAML enforcement/),
      ).toBeTruthy(),
    );
    const line = screen.getByText(/acme\/billing/).closest(".home__failed-repository");
    expect(line).toBeTruthy();
    expect(within(line as HTMLElement).getByRole("button", { name: "Remove" })).toBeTruthy();
  });

  it("removes the repository the failed line's Remove names", async () => {
    const user = userEvent.setup();
    refreshHome.mockResolvedValue(
      ok({
        ...listed(),
        failed_repositories: [
          { path: "/Developer/billing", slug: "acme/billing", reason: "GitHub refused" },
        ],
      }),
    );
    removeRepository.mockResolvedValue(ok(listed()));

    render(<App />);
    await waitFor(() => expect(screen.getByText(/acme\/billing/)).toBeTruthy());

    const line = screen.getByText(/acme\/billing/).closest(".home__failed-repository");
    await user.click(within(line as HTMLElement).getByRole("button", { name: "Remove" }));

    await waitFor(() => expect(removeRepository).toHaveBeenCalledWith("/Developer/billing"));
  });

  it("stamps the progress the refresh channel reports, then the time it settled", async () => {
    let progress: ((state: unknown) => void) | null = null;
    let settle: ((snapshot: unknown) => void) | null = null;
    refreshHome.mockImplementation((onProgress: { onmessage: (state: unknown) => void }) => {
      progress = (state: unknown) => onProgress.onmessage(state);
      return new Promise((resolve) => {
        settle = resolve;
      });
    });

    render(<App />);
    await waitFor(() => expect(progress).not.toBeNull());

    // The header goes up on the first report, so the window is never blank.
    act(() => progress?.({ Refreshing: { done: 0, total: 0 } }));
    await waitFor(() => expect(screen.getByText("Refreshing")).toBeTruthy());

    act(() => progress?.({ Refreshing: { done: 1, total: 3 } }));
    await waitFor(() => expect(screen.getByText("Refreshing 1 of 3")).toBeTruthy());

    act(() => settle?.(ok(listed())));

    await waitFor(() => expect(screen.getByText("Refreshed 2 min ago")).toBeTruthy());
    expect(screen.getByText("Retry webhook deliveries")).toBeTruthy();
  });

  it("refreshes on r, showing what the settings file now holds", async () => {
    const user = userEvent.setup();

    render(<App />);
    await waitFor(() => expect(screen.getByText("No repositories yet")).toBeTruthy());
    refreshHome.mockResolvedValue(ok(configured()));

    await user.keyboard("r");

    await waitFor(() => expect(screen.getByText("To review")).toBeTruthy());
    expect(screen.getByText("2 repositories · 1 failed")).toBeTruthy();
  });

  it("leaves the settings file alone when r is pressed with shift held", async () => {
    const user = userEvent.setup();

    render(<App />);
    await waitFor(() => expect(screen.getByText("No repositories yet")).toBeTruthy());
    refreshHome.mockClear();

    await user.keyboard("{Shift>}r{/Shift}");

    expect(refreshHome).not.toHaveBeenCalled();
  });

  it("expands the footer on e", async () => {
    const user = userEvent.setup();
    refreshHome.mockResolvedValue(ok(configured()));
    toggleRepositoriesFooter.mockResolvedValue({ ...configured(), footer_expanded: true });

    render(<App />);
    await waitFor(() => expect(screen.getByText("Repositories")).toBeTruthy());

    await user.keyboard("e");

    await waitFor(() => expect(screen.getByText("braidonw/zreview")).toBeTruthy());
  });

  it("shows the failure screen when a Home command rejects", async () => {
    refreshHome.mockReset();
    refreshHome.mockRejectedValue(new Error("IPC is unavailable"));

    render(<App />);

    await waitFor(() => expect(screen.getByText("Error: IPC is unavailable")).toBeTruthy());
  });

  it("shows the failure screen when a Home command answers with an error", async () => {
    refreshHome.mockResolvedValue({
      status: "error",
      error: { summary: "Home could not finish that action", detail: null, remediation: null },
    });

    render(<App />);

    await waitFor(() =>
      expect(screen.getByText("Home could not finish that action")).toBeTruthy(),
    );
  });

  it("shows the failure screen when the picker rejects", async () => {
    const user = userEvent.setup();
    open.mockRejectedValue(new Error("the picker could not open"));

    render(<App />);
    await waitFor(() => expect(screen.getByText("No repositories yet")).toBeTruthy());

    await user.click(screen.getByRole("button", { name: "Add repository..." }));

    await waitFor(() =>
      expect(screen.getByText("Error: the picker could not open")).toBeTruthy(),
    );
  });
});
