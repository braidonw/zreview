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

// jsdom never lays out elements, so stub the offsets the virtualizer reads.
Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
  configurable: true,
  value: 400,
});
Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
  configurable: true,
  value: 800,
});
