import { useEffect, useRef } from "react";
import { createMarkdownEditor, type MarkdownEditorHandle } from "../editor/markdownEditor";
import "./SummaryEditor.css";

/**
 * The review summary, in the same Markdown editor the comment composer uses.
 *
 * Seeded once on mount from whatever storage restored, and written through on
 * every keystroke. `summary` is reloaded into it only when it moves, which the
 * backend does after a whole-change finding lands in it or a successful send
 * empties it. Typing never moves it, so the reviewer's cursor is left alone.
 */
export function SummaryEditor({
  summary,
  onChange,
}: {
  summary: string;
  onChange: (body: string) => void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<MarkdownEditorHandle | null>(null);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const loadedRef = useRef(summary);
  const seedRef = useRef(summary);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) {
      return;
    }
    const handle = createMarkdownEditor({
      parent: container,
      initialText: seedRef.current,
      onChange: (body) => onChangeRef.current(body),
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
    if (summary === loadedRef.current) {
      return;
    }
    loadedRef.current = summary;
    editorRef.current?.load(summary);
  }, [summary]);

  return (
    <div className="summary-editor" data-summary-editor>
      <div className="summary-editor__field" ref={containerRef} />
    </div>
  );
}
