import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "./App";
import { makeHomeRepository, makeHomeSnapshot } from "./test/fixtures";

const describeLaunch = vi.fn();
const refreshHome = vi.fn();
const addRepositories = vi.fn();
const removeRepository = vi.fn();
const toggleRepositoriesFooter = vi.fn();
const dismissRefusals = vi.fn();

vi.mock("./bindings", () => ({
  commands: {
    describeLaunch: () => describeLaunch(),
    refreshHome: () => refreshHome(),
    addRepositories: (folders: unknown) => addRepositories(folders),
    removeRepository: (path: unknown) => removeRepository(path),
    toggleRepositoriesFooter: () => toggleRepositoriesFooter(),
    dismissRefusals: () => dismissRefusals(),
  },
}));

const open = vi.fn();
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (options: unknown) => open(options),
}));

/// Home with two clones configured, one of which did not resolve.
function configured() {
  return makeHomeSnapshot({
    count_line: "2 repositories",
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
  refreshHome.mockResolvedValue(makeHomeSnapshot());
  addRepositories.mockReset();
  removeRepository.mockReset();
  toggleRepositoriesFooter.mockReset();
  dismissRefusals.mockReset();
  open.mockReset();
});

describe("Home", () => {
  it("renders the heading, the count line, and the three groups with their empty copy", async () => {
    refreshHome.mockResolvedValue(configured());

    render(<App />);

    await waitFor(() => expect(screen.getByText("Home")).toBeTruthy());
    expect(screen.getByText("2 repositories")).toBeTruthy();
    expect(screen.getByText("To review")).toBeTruthy();
    expect(screen.getByText("To address")).toBeTruthy();
    expect(screen.getByText("Waiting on others")).toBeTruthy();
    expect(screen.getByText("Nothing waiting for your review.")).toBeTruthy();
    expect(screen.getByText("Nothing to address.")).toBeTruthy();
    expect(screen.getByText("Nothing waiting on others.")).toBeTruthy();
  });

  it("shows the footer collapsed to its summary line with Add beside it", async () => {
    refreshHome.mockResolvedValue(configured());

    render(<App />);

    await waitFor(() => expect(screen.getByText("Repositories")).toBeTruthy());
    expect(screen.getByText("2 repositories · 1 failed")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Add..." })).toBeTruthy();
    expect(screen.queryByText("/Developer/zreview")).toBeNull();
  });

  it("lists each repository's slug, path, state, and Remove once the footer expands", async () => {
    const user = userEvent.setup();
    refreshHome.mockResolvedValue(configured());
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
      makeHomeSnapshot({
        failure: {
          summary: "Home could not read your settings",
          detail: "could not parse the settings file",
          remediation: "Fix ~/.config/zreview/settings.toml, then press r to refresh.",
        },
      }),
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

  it("passes the picked folders to the add command and shows what it refused", async () => {
    const user = userEvent.setup();
    open.mockResolvedValue(["/Developer/notes", "/Developer/billing"]);
    addRepositories.mockResolvedValue(
      makeHomeSnapshot({
        count_line: "1 repository",
        repositories: [makeHomeRepository({ path: "/Developer/billing", slug: "acme/billing" })],
        footer_summary: "1 repository",
        refusals: [{ folder: "/Developer/notes", reason: "not a Git repository" }],
      }),
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
    refreshHome.mockResolvedValue({ ...configured(), footer_expanded: true });
    removeRepository.mockResolvedValue(makeHomeSnapshot({ footer_expanded: true }));

    render(<App />);
    await waitFor(() => expect(screen.getByText("braidonw/zreview")).toBeTruthy());

    await user.click(screen.getAllByRole("button", { name: "Remove" })[0]);

    expect(removeRepository).toHaveBeenCalledWith("/Developer/zreview");
  });

  it("refreshes on r", async () => {
    const user = userEvent.setup();
    render(<App />);
    await waitFor(() => expect(refreshHome).toHaveBeenCalledTimes(1));

    await user.keyboard("r");

    await waitFor(() => expect(refreshHome).toHaveBeenCalledTimes(2));
  });

  it("expands the footer on e", async () => {
    const user = userEvent.setup();
    refreshHome.mockResolvedValue(configured());
    toggleRepositoriesFooter.mockResolvedValue({ ...configured(), footer_expanded: true });

    render(<App />);
    await waitFor(() => expect(screen.getByText("Repositories")).toBeTruthy());

    await user.keyboard("e");

    await waitFor(() => expect(screen.getByText("braidonw/zreview")).toBeTruthy());
  });
});
