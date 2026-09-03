import { useEffect, useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { DraftsDto, RowDto } from "../bindings";
import type { ComposerState } from "../hooks/sessionReducer";
import { draftAtRow } from "../hooks/sessionReducer";
import { CommentComposer } from "./CommentComposer";
import { DiffRow } from "./DiffRow";
import { DraftCard } from "./DraftCard";
import { HunkHeader } from "./HunkHeader";
import "./DiffList.css";

export function DiffList({
  rows,
  isShowing,
  fileIndex,
  cursor,
  selectionStart,
  selectionEnd,
  drafts,
  composer,
  composerPrefill,
  onRowClick,
  onOpenComposer,
  onComposerChange,
  onComposerClose,
  onComposerDiscard,
}: {
  rows: RowDto[];
  /** False while Home is in front, which is when nothing here has a size. */
  isShowing: boolean;
  fileIndex: number;
  cursor: number;
  selectionStart: number;
  selectionEnd: number;
  drafts: DraftsDto;
  composer: ComposerState;
  composerPrefill: string;
  onRowClick: (index: number) => void;
  onOpenComposer: (index: number) => void;
  onComposerChange: (body: string) => void;
  onComposerClose: () => void;
  onComposerDiscard: () => void;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (index) => (rows[index]?.hunk_header ? 40 : 20),
    overscan: 20,
    getItemKey: (index) => index,
  });

  useEffect(() => {
    // Re-measure and reset scroll when the file changes.
    virtualizer.measure();
    if (rows.length > 0) {
      virtualizer.scrollToIndex(0, { align: "start" });
    }
  }, [fileIndex]);

  useEffect(() => {
    // Only the cursor moving should trigger a scroll.
    if (cursor < 0 || rows.length === 0) {
      return;
    }
    virtualizer.scrollToIndex(cursor, { align: "auto" });
  }, [cursor, virtualizer]);

  useEffect(() => {
    // Nothing has a size while the Session is hidden, so what was measured
    // then is worth nothing and the scroll it decided is gone. Measure again
    // and put the cursor row back where the reviewer left it.
    if (!isShowing || rows.length === 0) {
      return;
    }
    virtualizer.measure();
    virtualizer.scrollToIndex(cursor, { align: "auto" });
  }, [isShowing]);

  return (
    <div ref={scrollRef} className="diff-list">
      <div className="diff-list__spacer" style={{ height: virtualizer.getTotalSize() }}>
        {virtualizer.getVirtualItems().map((item) => {
          const row = rows[item.index];
          if (!row) {
            return null;
          }
          const draft = draftAtRow(drafts, item.index);
          const composerOpenHere = composer !== null && composer.rows[1] === item.index;
          return (
            <div
              key={item.key}
              data-index={item.index}
              ref={virtualizer.measureElement}
              className="diff-list__item"
              style={{ transform: `translateY(${item.start}px)` }}
            >
              {row.hunk_header !== null && <HunkHeader header={row.hunk_header} />}
              <DiffRow
                row={row}
                selected={item.index === cursor}
                inSelection={item.index >= selectionStart && item.index <= selectionEnd}
                draft={draft}
                showPill={item.index === cursor && !composerOpenHere}
                onClick={() => onRowClick(item.index)}
                onOpenComposer={() => onOpenComposer(item.index)}
              />
              {draft && !composerOpenHere && <DraftCard body={draft.body} />}
              {composerOpenHere && composer && (
                <CommentComposer
                  rows={composer.rows}
                  isShowing={isShowing}
                  prefill={composerPrefill}
                  notice={composer.notice}
                  onChange={onComposerChange}
                  onClose={onComposerClose}
                  onDiscard={onComposerDiscard}
                />
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
