import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { useNow } from "./useNow";

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-09-01T12:00:00Z"));
});

afterEach(() => {
  vi.useRealTimers();
});

describe("useNow", () => {
  it("starts at now and moves on once the minute is out", () => {
    const { result } = renderHook(() => useNow());
    const started = result.current;

    act(() => vi.advanceTimersByTime(30_000));
    expect(result.current).toBe(started);

    act(() => vi.advanceTimersByTime(30_000));
    expect(result.current).toBe(started + 60_000);
  });

  it("stops reading the clock once nothing is showing a time", () => {
    const { result, unmount } = renderHook(() => useNow());
    const started = result.current;

    unmount();
    act(() => vi.advanceTimersByTime(600_000));

    expect(result.current).toBe(started);
    expect(vi.getTimerCount()).toBe(0);
  });
});
