import { useEffect, useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { RowDto } from "../bindings";
import { DiffRow } from "./DiffRow";
import { HunkHeader } from "./HunkHeader";
import "./DiffList.css";

export function DiffList({
  rows,
  fileIndex,
  cursor,
  selectionStart,
  selectionEnd,
  onRowClick,
}: {
  rows: RowDto[];
  fileIndex: number;
  cursor: number;
  selectionStart: number;
  selectionEnd: number;
  onRowClick: (index: number) => void;
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

  return (
    <div ref={scrollRef} className="diff-list">
      <div className="diff-list__spacer" style={{ height: virtualizer.getTotalSize() }}>
        {virtualizer.getVirtualItems().map((item) => {
          const row = rows[item.index];
          if (!row) {
            return null;
          }
          return (
            <div
              key={item.key}
              className="diff-list__item"
              style={{ height: item.size, transform: `translateY(${item.start}px)` }}
            >
              {row.hunk_header !== null && <HunkHeader header={row.hunk_header} />}
              <DiffRow
                row={row}
                selected={item.index === cursor}
                inSelection={item.index >= selectionStart && item.index <= selectionEnd}
                onClick={() => onRowClick(item.index)}
              />
            </div>
          );
        })}
      </div>
    </div>
  );
}
