import { useEffect, useRef } from "react";
import { createMarkdownEditor } from "../editor/markdownEditor";
import "./CommentComposer.css";

/** The comment editor over a frozen row span, pre-filled silently on mount and rebuilt only when `rows` changes identity. */
export function CommentComposer({
  rows,
  prefill,
  notice,
  onChange,
  onClose,
  onDiscard,
}: {
  rows: [number, number];
  prefill: string;
  notice: string | null;
  onChange: (body: string) => void;
  onClose: () => void;
  onDiscard: () => void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    const container = containerRef.current;
    if (!container) {
      return;
    }
    const handle = createMarkdownEditor({
      parent: container,
      initialText: prefill,
      onChange: (body) => onChangeRef.current(body),
      onClose: () => onCloseRef.current(),
    });
    handle.focus();
    return () => handle.destroy();
    // Deliberately keyed on the span alone. Prefill only seeds the first mount.
  }, [rows[0], rows[1]]);

  return (
    <div className="comment-composer" data-composer>
      <div className="comment-composer__editor" ref={containerRef} />
      <div className="comment-composer__actions">
        <button type="button" className="comment-composer__done" onClick={onClose}>
          Done
        </button>
        <button type="button" className="comment-composer__discard" onClick={onDiscard}>
          Discard
        </button>
      </div>
      {notice !== null && <div className="comment-composer__notice">{notice}</div>}
    </div>
  );
}
