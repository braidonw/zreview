import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";

afterEach(() => {
  cleanup();
});

// Stub the Tauri runtime bridge a `Channel` needs to construct without a real webview.
let nextCallbackId = 0;
Object.defineProperty(window, "__TAURI_INTERNALS__", {
  configurable: true,
  value: {
    transformCallback: () => {
      nextCallbackId += 1;
      return nextCallbackId;
    },
  },
});

// The virtualizer's measureElement ref reads offsetHeight synchronously on
// mount, regardless of ResizeObserver, so every item needs a plausible one.
// Test nominals, not layout truth. Kept consistent with estimateSize below.
const ROW_ITEM_HEIGHT = 20;
const HUNK_HEADER_ITEM_HEIGHT = 40;
const COMPOSER_NOMINAL_HEIGHT = 120;
const DRAFT_CARD_NOMINAL_HEIGHT = 60;

Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
  configurable: true,
  get(this: HTMLElement) {
    // Every other element keeps the flat stub, e.g. the scroll container.
    if (!this.classList.contains("diff-list__item")) {
      return 400;
    }
    let height = this.querySelector(".hunk-header") ? HUNK_HEADER_ITEM_HEIGHT : ROW_ITEM_HEIGHT;
    if (this.querySelector("[data-composer]")) {
      height += COMPOSER_NOMINAL_HEIGHT;
    }
    if (this.querySelector(".draft-card")) {
      height += DRAFT_CARD_NOMINAL_HEIGHT;
    }
    return height;
  },
});
Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
  configurable: true,
  value: 800,
});

// jsdom has no layout engine, and CodeMirror measures text with these during
// construction and on every document change.
const stubRect: DOMRect = {
  x: 0,
  y: 0,
  width: 0,
  height: 0,
  top: 0,
  left: 0,
  right: 0,
  bottom: 0,
  toJSON: () => ({}),
};
Range.prototype.getClientRects = () => [stubRect] as unknown as DOMRectList;
Range.prototype.getBoundingClientRect = () => stubRect;
Element.prototype.getClientRects = () => [stubRect] as unknown as DOMRectList;
Element.prototype.getBoundingClientRect = () => stubRect;

// jsdom has no scroll machinery, so the cursor row's scrollIntoView is not there to call.
Element.prototype.scrollIntoView = () => {};

// jsdom has no ResizeObserver at all; CodeMirror's view and the virtualizer both construct one.
class NoOpResizeObserver implements ResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
window.ResizeObserver = NoOpResizeObserver;
