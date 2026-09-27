import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it } from "vitest";
import { Toaster } from "./Toaster";
import { useToasts } from "../state/toast";

beforeEach(() => useToasts.setState({ toasts: [] }));
afterEach(() => {
    for (const toast of useToasts.getState().toasts) useToasts.getState().dismiss(toast.id);
    cleanup();
});

it("offers a dismiss button only on toasts that stay until dismissed", () => {
    render(<Toaster />);
    act(() => useToasts.getState().push("success", "copied relative path"));
    expect(screen.queryByRole("button", { name: "Dismiss notification" })).toBeNull();

    act(() => useToasts.getState().push("error", "open file: No such file or directory (os error 2)"));
    expect(screen.getAllByRole("button", { name: "Dismiss notification" })).toHaveLength(1);
});

it("keeps the action beside the dismiss button on a toast that waits for it", () => {
    render(<Toaster />);
    act(() => useToasts.getState().push("error", "Toaster.tsx has an external change.", { action: { label: "Reload disk", run: () => {} } }));
    expect(screen.getByRole("button", { name: "Reload disk" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Dismiss notification" })).toBeTruthy();
});
