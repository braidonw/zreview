import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import type { RowDto } from "../bindings";
import { makeRow } from "../test/fixtures";
import { DiffList } from "./DiffList";

const ROW_COUNT = 200;

function rows(): RowDto[] {
  return Array.from({ length: ROW_COUNT }, (_, index) =>
    makeRow({
      text: `line ${index}`,
      hunk_header: index === 0 ? "@@ -1,200 +1,200 @@" : null,
    }),
  );
}

describe("DiffList", () => {
  it("renders a window around the cursor, not all 200 rows", () => {
    render(
      <DiffList
        rows={rows()}
        fileIndex={0}
        cursor={25}
        selectionStart={25}
        selectionEnd={25}
        onRowClick={() => {}}
      />,
    );

    const rendered = screen.getAllByText(/^line \d+$/);
    expect(rendered.length).toBeGreaterThan(0);
    expect(rendered.length).toBeLessThan(ROW_COUNT);
    expect(screen.getByText("line 25")).toBeTruthy();
  });

  it("renders the first row's hunk header at 40px", () => {
    render(
      <DiffList
        rows={rows()}
        fileIndex={0}
        cursor={0}
        selectionStart={0}
        selectionEnd={0}
        onRowClick={() => {}}
      />,
    );

    const header = screen.getByText("@@ -1,200 +1,200 @@");
    const item = header.closest(".diff-list__item");
    expect(item).not.toBeNull();
    expect((item as HTMLElement).style.height).toBe("40px");
  });

  it("styles the row at the cursor as selected", () => {
    render(
      <DiffList
        rows={rows()}
        fileIndex={0}
        cursor={5}
        selectionStart={5}
        selectionEnd={5}
        onRowClick={() => {}}
      />,
    );

    const cursorRow = screen.getByText("line 5").closest(".diff-row");
    const otherRow = screen.getByText("line 6").closest(".diff-row");
    expect(cursorRow?.className).toContain("diff-row--selected");
    expect(otherRow?.className).not.toContain("diff-row--selected");
  });
});
