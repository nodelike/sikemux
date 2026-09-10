import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useToasts } from "./toast";
beforeEach(() => {
    vi.useFakeTimers();
    useToasts.setState({ toasts: [] });
});
afterEach(() => {
    for (const toast of useToasts.getState().toasts) useToasts.getState().dismiss(toast.id);
    vi.useRealTimers();
});
it("keeps errors until dismissed", () => {
    useToasts.getState().push("error", "Commit rejected");
    vi.advanceTimersByTime(60_000);
    expect(useToasts.getState().toasts).toHaveLength(1);
});
it("pauses timeout while hovered or focused, then resumes remaining time", () => {
    const store = useToasts.getState();
    store.push("success", "Saved");
    const id = useToasts.getState().toasts[0].id;
    vi.advanceTimersByTime(2000);
    store.pause(id, "pointer");
    store.pause(id, "focus");
    vi.advanceTimersByTime(10_000);
    store.resume(id, "pointer");
    vi.advanceTimersByTime(10_000);
    expect(useToasts.getState().toasts).toHaveLength(1);
    store.resume(id, "focus");
    vi.advanceTimersByTime(2999);
    expect(useToasts.getState().toasts).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(useToasts.getState().toasts).toHaveLength(0);
});
