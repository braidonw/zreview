import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { markdown } from "@codemirror/lang-markdown";
import { EditorState } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";

/** What a component gets back from the composer's editor; components never import CodeMirror directly. */
export interface MarkdownEditorHandle {
  focus: () => void;
  destroy: () => void;
}

/** Builds a CodeMirror 6 Markdown editor mounted on `parent`, saving on every change and closing on Escape or Mod-Enter. */
export function createMarkdownEditor({
  parent,
  initialText,
  onChange,
  onClose,
}: {
  parent: HTMLElement;
  initialText: string;
  onChange: (text: string) => void;
  onClose: () => void;
}): MarkdownEditorHandle {
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc: initialText,
      extensions: [
        markdown(),
        history(),
        keymap.of([
          {
            key: "Escape",
            run: () => {
              onClose();
              return true;
            },
          },
          {
            key: "Mod-Enter",
            run: () => {
              onClose();
              return true;
            },
          },
          ...defaultKeymap,
          ...historyKeymap,
        ]),
        EditorView.lineWrapping,
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            onChange(update.state.doc.toString());
          }
        }),
        EditorView.theme({
          "&": {
            height: "100%",
            fontFamily: "var(--font-sans)",
            fontSize: "var(--size-body)",
            color: "var(--text-primary)",
            backgroundColor: "var(--surface-overlay)",
          },
          ".cm-scroller": {
            fontFamily: "var(--font-sans)",
          },
          ".cm-content": {
            caretColor: "var(--accent-base)",
          },
          "&.cm-focused .cm-cursor": {
            borderLeftColor: "var(--accent-base)",
          },
          "&.cm-focused .cm-selectionBackground, .cm-selectionBackground": {
            backgroundColor: "var(--surface-selected)",
          },
          ".cm-gutters": {
            display: "none",
          },
        }),
      ],
    }),
  });

  return {
    focus: () => view.focus(),
    destroy: () => view.destroy(),
  };
}
