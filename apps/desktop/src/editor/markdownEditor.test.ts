import { afterEach, describe, expect, it, vi } from "vitest";
import { createMarkdownEditor, type MarkdownEditorHandle } from "./markdownEditor";

let handle: MarkdownEditorHandle | undefined;

afterEach(() => {
  handle?.destroy();
  handle = undefined;
});

describe("createMarkdownEditor", () => {
  it("mounts with the initial text rendered", () => {
    const parent = document.createElement("div");
    handle = createMarkdownEditor({
      parent,
      initialText: "hello",
      onChange: () => {},
      onClose: () => {},
    });

    expect(parent.querySelector(".cm-editor")).not.toBeNull();
    expect(parent.querySelector(".cm-content")?.textContent).toBe("hello");
  });

  it("calls onClose without discarding the document on Escape", () => {
    const parent = document.createElement("div");
    const onClose = vi.fn();
    handle = createMarkdownEditor({
      parent,
      initialText: "keep me",
      onChange: () => {},
      onClose,
    });

    const content = parent.querySelector(".cm-content") as HTMLElement;
    content.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));

    expect(onClose).toHaveBeenCalledTimes(1);
    expect(content.textContent).toBe("keep me");
  });

  it("calls onClose without discarding the document on Mod-Enter", () => {
    const parent = document.createElement("div");
    const onClose = vi.fn();
    handle = createMarkdownEditor({
      parent,
      initialText: "keep me too",
      onChange: () => {},
      onClose,
    });

    // jsdom reports no platform, so CodeMirror resolves "Mod" to Ctrl here.
    const content = parent.querySelector(".cm-content") as HTMLElement;
    content.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", ctrlKey: true, bubbles: true }),
    );

    expect(onClose).toHaveBeenCalledTimes(1);
    expect(content.textContent).toBe("keep me too");
  });

  it("destroys cleanly, detaching from the parent", () => {
    const parent = document.createElement("div");
    handle = createMarkdownEditor({
      parent,
      initialText: "",
      onChange: () => {},
      onClose: () => {},
    });

    handle.destroy();
    handle = undefined;

    expect(parent.querySelector(".cm-editor")).toBeNull();
  });
});
