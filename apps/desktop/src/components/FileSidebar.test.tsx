import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { FileStatusDto } from "../bindings";
import { makeFileSummary, makeSidebar } from "../test/fixtures";
import { FileSidebar } from "./FileSidebar";

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
      const { unmount } = render(
        <FileSidebar title="t" subtitle="s" sidebar={makeSidebar(files)} onSelect={() => {}} />,
      );

      expect(screen.getByText(glyph).className).toContain(className);
      unmount();
    }
  });

  it("shows add and delete counts for a text file", () => {
    const files = [makeFileSummary({ additions: 12, deletions: 3 })];
    render(<FileSidebar title="t" subtitle="s" sidebar={makeSidebar(files)} onSelect={() => {}} />);

    expect(screen.getByText("+12")).toBeTruthy();
    expect(screen.getByText("-3")).toBeTruthy();
  });

  it("shows 'binary' instead of counts for a binary file", () => {
    const files = [makeFileSummary({ is_binary: true, additions: 5, deletions: 5 })];
    render(<FileSidebar title="t" subtitle="s" sidebar={makeSidebar(files)} onSelect={() => {}} />);

    expect(screen.getByText("binary")).toBeTruthy();
    expect(screen.queryByText("+5")).toBeNull();
  });

  it("marks a viewed file with a checkmark", () => {
    const files = [makeFileSummary({ viewed: true })];
    render(<FileSidebar title="t" subtitle="s" sidebar={makeSidebar(files)} onSelect={() => {}} />);

    expect(screen.getByText("✓")).toBeTruthy();
  });

  it("hides the thread badge when a file has no threads", () => {
    const files = [makeFileSummary({ thread_count: 0 })];
    render(<FileSidebar title="t" subtitle="s" sidebar={makeSidebar(files)} onSelect={() => {}} />);

    expect(screen.queryByText("0")).toBeNull();
  });

  it("shows the thread badge when a file has threads", () => {
    const files = [makeFileSummary({ thread_count: 3 })];
    render(<FileSidebar title="t" subtitle="s" sidebar={makeSidebar(files)} onSelect={() => {}} />);

    expect(screen.getByText("3")).toBeTruthy();
  });

  it("calls onSelect with the clicked file's index", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    const files = [
      makeFileSummary({ index: 0, path: "a.rs" }),
      makeFileSummary({ index: 1, path: "b.rs" }),
    ];
    render(
      <FileSidebar title="t" subtitle="s" sidebar={makeSidebar(files)} onSelect={onSelect} />,
    );

    await user.click(screen.getByText("b.rs"));

    expect(onSelect).toHaveBeenCalledWith(1);
  });
});
