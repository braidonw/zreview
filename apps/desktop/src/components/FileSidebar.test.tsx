import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { FileStatusDto } from "../bindings";
import { makeFileSummary, makeSidebar } from "../test/fixtures";
import { FileSidebar } from "./FileSidebar";

/** Every prop FileSidebar needs beyond the ones a test wants to vary. */
function baseProps() {
  return {
    title: "t",
    subtitle: "s",
    warnings: [],
    writeFailure: null,
    fileDraftCount: 0,
    staleDrafts: [],
    cursor: 0,
    onSelect: () => {},
    onReanchorDraft: () => {},
  };
}

describe("FileSidebar", () => {
  it("maps every status to a distinct glyph and colour class", () => {
    const expectations: { status: FileStatusDto; glyph: string; className: string }[] = [
      { status: "Added", glyph: "A", className: "file-row__status--success" },
      { status: "Deleted", glyph: "D", className: "file-row__status--error" },
      { status: "Modified", glyph: "M", className: "file-row__status--warning" },
      { status: "Renamed", glyph: "R", className: "file-row__status--info" },
      { status: "Copied", glyph: "C", className: "file-row__status--proposed" },
      { status: "TypeChanged", glyph: "T", className: "file-row__status--warning" },
      { status: "Unmerged", glyph: "U", className: "file-row__status--error-strong" },
    ];

    for (const { status, glyph, className } of expectations) {
      const files = [makeFileSummary({ status })];
      const { unmount } = render(<FileSidebar {...baseProps()} sidebar={makeSidebar(files)} />);

      expect(screen.getByText(glyph).className).toContain(className);
      unmount();
    }
  });

  it("shows add and delete counts for a text file", () => {
    const files = [makeFileSummary({ additions: 12, deletions: 3 })];
    render(<FileSidebar {...baseProps()} sidebar={makeSidebar(files)} />);

    expect(screen.getByText("+12")).toBeTruthy();
    expect(screen.getByText("-3")).toBeTruthy();
  });

  it("shows 'binary' instead of counts for a binary file", () => {
    const files = [makeFileSummary({ is_binary: true, additions: 5, deletions: 5 })];
    render(<FileSidebar {...baseProps()} sidebar={makeSidebar(files)} />);

    expect(screen.getByText("binary")).toBeTruthy();
    expect(screen.queryByText("+5")).toBeNull();
  });

  it("marks a viewed file with a checkmark", () => {
    const files = [makeFileSummary({ viewed: true })];
    render(<FileSidebar {...baseProps()} sidebar={makeSidebar(files)} />);

    expect(screen.getByText("✓")).toBeTruthy();
  });

  it("hides the thread badge when a file has no threads", () => {
    const files = [makeFileSummary({ thread_count: 0 })];
    render(<FileSidebar {...baseProps()} sidebar={makeSidebar(files)} />);

    expect(screen.queryByText("0")).toBeNull();
  });

  it("shows the thread badge when a file has threads", () => {
    const files = [makeFileSummary({ thread_count: 3 })];
    render(<FileSidebar {...baseProps()} sidebar={makeSidebar(files)} />);

    expect(screen.getByText("3")).toBeTruthy();
  });

  it("calls onSelect with the clicked file's index", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    const files = [
      makeFileSummary({ index: 0, path: "a.rs" }),
      makeFileSummary({ index: 1, path: "b.rs" }),
    ];
    render(<FileSidebar {...baseProps()} sidebar={makeSidebar(files)} onSelect={onSelect} />);

    await user.click(screen.getByText("b.rs"));

    expect(onSelect).toHaveBeenCalledWith(1);
  });

  it("shows no warnings strip when there is nothing to warn about", () => {
    render(<FileSidebar {...baseProps()} sidebar={makeSidebar()} />);

    expect(document.querySelector(".file-sidebar__warnings")).toBeNull();
  });

  it("shows a warning from the session's own warnings list", () => {
    render(
      <FileSidebar
        {...baseProps()}
        sidebar={makeSidebar()}
        warnings={[{ summary: "GitHub's rate limit is exhausted", detail: null, remediation: null }]}
      />,
    );

    expect(screen.getByText("GitHub's rate limit is exhausted")).toBeTruthy();
  });

  it("merges the sink's write failure in alongside the session's own warnings", () => {
    render(
      <FileSidebar
        {...baseProps()}
        sidebar={makeSidebar()}
        warnings={[{ summary: "1 saved draft no longer matches this diff", detail: null, remediation: null }]}
        writeFailure="Drafts are not being saved"
      />,
    );

    expect(screen.getByText("1 saved draft no longer matches this diff")).toBeTruthy();
    expect(screen.getByText("Drafts are not being saved")).toBeTruthy();
  });

  it("hides the drafts panel when the file has no drafts", () => {
    render(<FileSidebar {...baseProps()} sidebar={makeSidebar()} fileDraftCount={0} />);

    expect(screen.queryByText(/of your drafts/)).toBeNull();
  });

  it("shows the drafts panel's count once the file has a draft", () => {
    render(<FileSidebar {...baseProps()} sidebar={makeSidebar()} fileDraftCount={2} />);

    expect(screen.getByText("2 of your drafts")).toBeTruthy();
  });

  it("invokes onReanchorDraft with the stale draft's key and the current cursor", async () => {
    const user = userEvent.setup();
    const onReanchorDraft = vi.fn();
    render(
      <FileSidebar
        {...baseProps()}
        sidebar={makeSidebar()}
        fileDraftCount={1}
        staleDrafts={[
          {
            path: "src/review.rs",
            side: "Right",
            line: 42,
            body: "left last week",
            location: "was RIGHT line 42",
          },
        ]}
        cursor={9}
        onReanchorDraft={onReanchorDraft}
      />,
    );

    await user.click(screen.getByText("Move to row 10"));

    expect(onReanchorDraft).toHaveBeenCalledWith("src/review.rs", "Right", 42, 9);
  });
});
