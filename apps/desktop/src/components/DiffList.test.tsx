import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import type { RowDto } from "../bindings";
import type { ComposerState } from "../hooks/sessionReducer";
import { makeAnchoredDraft, makeDrafts, makeRow } from "../test/fixtures";
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

/** Every prop DiffList needs beyond the ones a test wants to vary. */
function baseProps() {
  return {
    fileIndex: 0,
    drafts: makeDrafts(),
    composer: null as ComposerState,
    composerPrefill: "",
    onRowClick: () => {},
    onOpenComposer: () => {},
    onComposerChange: () => {},
    onComposerClose: () => {},
    onComposerDiscard: () => {},
  };
}

describe("DiffList", () => {
  it("renders a window around the cursor, not all 200 rows", () => {
    render(
      <DiffList {...baseProps()} rows={rows()} cursor={25} selectionStart={25} selectionEnd={25} />,
    );

    const rendered = screen.getAllByText(/^line \d+$/);
    expect(rendered.length).toBeGreaterThan(0);
    expect(rendered.length).toBeLessThan(ROW_COUNT);
    expect(screen.getByText("line 25")).toBeTruthy();
  });

  it("renders the first row's hunk header above its row, in the same item", () => {
    render(<DiffList {...baseProps()} rows={rows()} cursor={0} selectionStart={0} selectionEnd={0} />);

    const header = screen.getByText("@@ -1,200 +1,200 @@");
    const item = header.closest(".diff-list__item");
    expect(item).not.toBeNull();
    // Semantic, not pixel. The item now sizes to its content (see test/setup.ts).
    expect(item?.querySelector(".diff-row")).not.toBeNull();
    expect(screen.getByText("line 0")).toBeTruthy();
  });

  it("styles the row at the cursor as selected", () => {
    render(<DiffList {...baseProps()} rows={rows()} cursor={5} selectionStart={5} selectionEnd={5} />);

    const cursorRow = screen.getByText("line 5").closest(".diff-row");
    const otherRow = screen.getByText("line 6").closest(".diff-row");
    expect(cursorRow?.className).toContain("diff-row--selected");
    expect(otherRow?.className).not.toContain("diff-row--selected");
  });

  it("shows a Comment pill at the cursor, and Edit when that row already has a draft", () => {
    const { rerender } = render(
      <DiffList {...baseProps()} rows={rows()} cursor={5} selectionStart={5} selectionEnd={5} />,
    );
    expect(screen.getByText("Comment")).toBeTruthy();

    rerender(
      <DiffList
        {...baseProps()}
        rows={rows()}
        cursor={5}
        selectionStart={5}
        selectionEnd={5}
        drafts={makeDrafts({ anchored: [makeAnchoredDraft({ row: 5, body: "existing" })] })}
      />,
    );
    expect(screen.getByText("Edit")).toBeTruthy();
  });

  it("does not show the pill on a row that is not the cursor", () => {
    render(<DiffList {...baseProps()} rows={rows()} cursor={5} selectionStart={5} selectionEnd={5} />);

    const other = screen.getByText("line 6").closest(".diff-list__item");
    expect(other?.querySelector(".diff-row__pill")).toBeNull();
  });

  it("clicking the pill opens the composer at that row", () => {
    const onOpenComposer = vi.fn();
    render(
      <DiffList
        {...baseProps()}
        rows={rows()}
        cursor={5}
        selectionStart={5}
        selectionEnd={5}
        onOpenComposer={onOpenComposer}
      />,
    );

    screen.getByText("Comment").click();

    expect(onOpenComposer).toHaveBeenCalledWith(5);
  });

  it("renders a resting draft card under its row, with the body visible", () => {
    render(
      <DiffList
        {...baseProps()}
        rows={rows()}
        cursor={0}
        selectionStart={0}
        selectionEnd={0}
        drafts={makeDrafts({ anchored: [makeAnchoredDraft({ row: 5, body: "worth a look" })] })}
      />,
    );

    expect(screen.getByText("Your draft")).toBeTruthy();
    expect(screen.getByText("worth a look")).toBeTruthy();
  });

  it("shows the composer instead of the draft card on the row it is open over", () => {
    render(
      <DiffList
        {...baseProps()}
        rows={rows()}
        cursor={5}
        selectionStart={5}
        selectionEnd={5}
        drafts={makeDrafts({ anchored: [makeAnchoredDraft({ row: 5, body: "editing this" })] })}
        composer={{ rows: [5, 5], notice: null }}
      />,
    );

    expect(screen.queryByText("Your draft")).toBeNull();
    expect(document.querySelector("[data-composer]")).not.toBeNull();
  });

  it("shows the rejected-span notice under the open composer", () => {
    render(
      <DiffList
        {...baseProps()}
        rows={rows()}
        cursor={5}
        selectionStart={5}
        selectionEnd={5}
        composer={{ rows: [5, 5], notice: "This selection cannot hold a comment" }}
      />,
    );

    expect(screen.getByText("This selection cannot hold a comment")).toBeTruthy();
  });
});
