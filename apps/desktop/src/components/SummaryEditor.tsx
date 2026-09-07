import { useEffect, useRef } from "react";
import { createMarkdownEditor, type MarkdownEditorHandle } from "../editor/markdownEditor";
import "./SummaryEditor.css";

/**
 * The review summary, in the same Markdown editor the comment composer uses.
 *
 * Seeded once on mount from whatever storage restored, and reported on every
 * keystroke. `body` is taken back into it only when `loads` moves, which the
 * backend does after a whole-change finding is merged in or a landed review
 * empties it. Typing never moves `loads`, so the reviewer's caret is never reset
 * by their own keystrokes.
 */
export function SummaryEditor({
  body,
  loads,
  onChange,
}: {
  body: string;
  /** How many times the backend has replaced the text. */
  loads: number;
  onChange: (body: string) => void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<MarkdownEditorHandle | null>(null);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const loadedRef = useRef(loads);
  const seedRef = useRef(body);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) {
      return;
    }
    const handle = createMarkdownEditor({
      parent: container,
      initialText: seedRef.current,
      onChange: (typed) => onChangeRef.current(typed),
      // The summary has nothing to close; Escape and Mod-Enter do nothing here.
      onClose: () => {},
    });
    editorRef.current = handle;
    return () => {
      editorRef.current = null;
      handle.destroy();
    };
  }, []);

  useEffect(() => {
    if (loads === loadedRef.current) {
      return;
    }
    loadedRef.current = loads;
    editorRef.current?.load(body);
  }, [loads, body]);

  return (
    <div className="summary-editor" data-summary-editor>
      <div className="summary-editor__field" ref={containerRef} />
    </div>
  );
}
