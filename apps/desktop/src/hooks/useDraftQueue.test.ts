import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { makeDrafts } from "../test/fixtures";
import { useDraftQueue } from "./useDraftQueue";

const editDraft = vi.fn();
const discardDraft = vi.fn();
const reanchorDraft = vi.fn();

vi.mock("../bindings", () => ({
  commands: {
    editDraft: (...args: unknown[]) => editDraft(...args),
    discardDraft: (...args: unknown[]) => discardDraft(...args),
    reanchorDraft: (...args: unknown[]) => reanchorDraft(...args),
  },
}));

beforeEach(() => {
  editDraft.mockReset();
  discardDraft.mockReset();
  reanchorDraft.mockReset();
});

/** A promise this test resolves by hand, standing in for a slow IPC round trip. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((givenResolve, givenReject) => {
    resolve = givenResolve;
    reject = givenReject;
  });
  return { promise, resolve, reject };
}

describe("useDraftQueue", () => {
  it("sends the first edit immediately and dispatches drafts once it settles", async () => {
    const dispatch = vi.fn();
    const first = deferred<unknown>();
    editDraft.mockReturnValueOnce(first.promise);
    const { result } = renderHook(() => useDraftQueue(dispatch));

    act(() => result.current.editDraft(0, 0, 0, "a"));

    expect(editDraft).toHaveBeenCalledExactlyOnceWith(0, 0, 0, "a");

    const drafts = makeDrafts();
    await act(async () => first.resolve({ status: "ok", data: { accepted: true, drafts } }));

    expect(dispatch).toHaveBeenCalledWith({ type: "drafts", drafts });
    expect(dispatch).not.toHaveBeenCalledWith({ type: "editRejected" });
  });

  it("coalesces edits queued while one is in flight, sending only the latest body", async () => {
    const dispatch = vi.fn();
    const first = deferred<unknown>();
    editDraft.mockReturnValueOnce(first.promise);
    const { result } = renderHook(() => useDraftQueue(dispatch));

    act(() => {
      result.current.editDraft(0, 0, 0, "a");
      result.current.editDraft(0, 0, 0, "ab");
      result.current.editDraft(0, 0, 0, "abc");
    });

    // The two intermediate keystrokes never reached the backend.
    expect(editDraft).toHaveBeenCalledTimes(1);

    const second = deferred<unknown>();
    editDraft.mockReturnValueOnce(second.promise);
    await act(async () =>
      first.resolve({ status: "ok", data: { accepted: true, drafts: makeDrafts() } }),
    );

    expect(editDraft).toHaveBeenCalledTimes(2);
    expect(editDraft).toHaveBeenLastCalledWith(0, 0, 0, "abc");

    await act(async () =>
      second.resolve({ status: "ok", data: { accepted: true, drafts: makeDrafts() } }),
    );
  });

  // A discard behind a queued edit is never coalesced away: both are sent, in order.
  it("sends a queued edit before the discard that follows it, never dropping either", async () => {
    const dispatch = vi.fn();
    const inFlightEdit = deferred<unknown>();
    editDraft.mockReturnValueOnce(inFlightEdit.promise);
    const { result } = renderHook(() => useDraftQueue(dispatch));

    act(() => {
      // The first edit runs immediately; the second queues behind it, and
      // the discard queues behind that.
      result.current.editDraft(0, 0, 0, "in flight already");
      result.current.editDraft(0, 0, 0, "typed while distracted");
      result.current.discardDraft(0, 0);
    });

    expect(editDraft).toHaveBeenCalledTimes(1);
    expect(discardDraft).not.toHaveBeenCalled();

    const queuedEditResponse = deferred<unknown>();
    editDraft.mockReturnValueOnce(queuedEditResponse.promise);
    await act(async () =>
      inFlightEdit.resolve({ status: "ok", data: { accepted: true, drafts: makeDrafts() } }),
    );

    // The queued edit was sent next, not dropped.
    expect(editDraft).toHaveBeenCalledTimes(2);
    expect(discardDraft).not.toHaveBeenCalled();

    const discardResponse = deferred<unknown>();
    discardDraft.mockReturnValueOnce(discardResponse.promise);
    await act(async () =>
      queuedEditResponse.resolve({ status: "ok", data: { accepted: true, drafts: makeDrafts() } }),
    );

    // Only now does the discard go out, landing last.
    expect(discardDraft).toHaveBeenCalledExactlyOnceWith(0, 0);

    await act(async () =>
      discardResponse.resolve({ status: "ok", data: makeDrafts({ file_draft_count: 0 }) }),
    );
  });

  // Discard, then reopen and type again: the discard must still be sent.
  it("still sends a discard queued behind an in-flight edit, even once a new edit follows it", async () => {
    const dispatch = vi.fn();
    const inFlightEdit = deferred<unknown>();
    editDraft.mockReturnValueOnce(inFlightEdit.promise);
    const { result } = renderHook(() => useDraftQueue(dispatch));

    act(() => {
      result.current.editDraft(0, 0, 0, "first draft");
      result.current.discardDraft(0, 0);
      // Reopened the composer and typed again, all before anything settled.
      result.current.editDraft(0, 0, 0, "second thoughts");
    });

    expect(editDraft).toHaveBeenCalledTimes(1);

    const discardResponse = deferred<unknown>();
    discardDraft.mockReturnValueOnce(discardResponse.promise);
    await act(async () =>
      inFlightEdit.resolve({ status: "ok", data: { accepted: true, drafts: makeDrafts() } }),
    );

    // The discard is next, not the later edit.
    expect(discardDraft).toHaveBeenCalledExactlyOnceWith(0, 0);
    expect(editDraft).toHaveBeenCalledTimes(1);

    const secondEditResponse = deferred<unknown>();
    editDraft.mockReturnValueOnce(secondEditResponse.promise);
    await act(async () =>
      discardResponse.resolve({ status: "ok", data: makeDrafts({ file_draft_count: 0 }) }),
    );

    // Finally, the edit that followed the discard.
    expect(editDraft).toHaveBeenCalledTimes(2);
    expect(editDraft).toHaveBeenLastCalledWith(0, 0, 0, "second thoughts");

    await act(async () =>
      secondEditResponse.resolve({ status: "ok", data: { accepted: true, drafts: makeDrafts() } }),
    );
  });

  it("dispatches editRejected only when the outcome was not accepted", async () => {
    const dispatch = vi.fn();
    const response = deferred<unknown>();
    editDraft.mockReturnValueOnce(response.promise);
    const { result } = renderHook(() => useDraftQueue(dispatch));

    act(() => result.current.editDraft(0, 0, 2, "spans a hunk boundary"));
    const drafts = makeDrafts();
    await act(async () => response.resolve({ status: "ok", data: { accepted: false, drafts } }));

    expect(dispatch).toHaveBeenCalledWith({ type: "drafts", drafts });
    expect(dispatch).toHaveBeenCalledWith({ type: "editRejected" });
  });

  it("drops a failed draft command silently rather than dispatching a fatal failure", async () => {
    const dispatch = vi.fn();
    const response = deferred<unknown>();
    discardDraft.mockReturnValueOnce(response.promise);
    const { result } = renderHook(() => useDraftQueue(dispatch));

    act(() => result.current.discardDraft(0, 0));
    await act(async () =>
      response.resolve({
        status: "error",
        error: { summary: "the session is not ready", detail: null, remediation: null },
      }),
    );

    expect(dispatch).not.toHaveBeenCalled();
  });

  // A rejected send, not just an error response, must not wedge the queue.
  it("un-wedges the queue when a send rejects outright", async () => {
    const dispatch = vi.fn();
    const failing = deferred<unknown>();
    editDraft.mockReturnValueOnce(failing.promise);
    const { result } = renderHook(() => useDraftQueue(dispatch));

    act(() => {
      result.current.editDraft(0, 0, 0, "a");
      result.current.editDraft(0, 0, 0, "ab");
    });

    const next = deferred<unknown>();
    editDraft.mockReturnValueOnce(next.promise);
    await act(async () => failing.reject(new Error("IPC channel closed")));

    // The queued edit was still sent: the rejection did not wedge the queue.
    expect(editDraft).toHaveBeenCalledTimes(2);
    expect(editDraft).toHaveBeenLastCalledWith(0, 0, 0, "ab");
    expect(dispatch).not.toHaveBeenCalled();

    await act(async () =>
      next.resolve({ status: "ok", data: { accepted: true, drafts: makeDrafts() } }),
    );
    expect(dispatch).toHaveBeenCalledWith({ type: "drafts", drafts: makeDrafts() });
  });

  it("reanchor and discard both take the queue path, not sending until earlier work settles", async () => {
    const dispatch = vi.fn();
    const inFlightEdit = deferred<unknown>();
    editDraft.mockReturnValueOnce(inFlightEdit.promise);
    const { result } = renderHook(() => useDraftQueue(dispatch));

    act(() => {
      result.current.editDraft(0, 0, 0, "note");
      result.current.reanchorDraft(0, "src/review.rs", "Right", 42, 3);
    });

    expect(reanchorDraft).not.toHaveBeenCalled();

    const reanchorResponse = deferred<unknown>();
    reanchorDraft.mockReturnValueOnce(reanchorResponse.promise);
    await act(async () =>
      inFlightEdit.resolve({ status: "ok", data: { accepted: true, drafts: makeDrafts() } }),
    );

    expect(reanchorDraft).toHaveBeenCalledExactlyOnceWith(0, "src/review.rs", "Right", 42, 3);

    await act(async () => reanchorResponse.resolve({ status: "ok", data: makeDrafts() }));
  });
});
