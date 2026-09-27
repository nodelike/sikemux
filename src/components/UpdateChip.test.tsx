import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { getState, setState } from "../state/store";
import { UpdateChip } from "./TopBar";

const initial = getState();

function showChip(overrides: Partial<NonNullable<ReturnType<typeof getState>["pendingUpdate"]>>) {
    setState({
        pendingUpdate: {
            version: "0.4.0-nightly.1",
            currentVersion: "0.3.5",
            notes: null,
            date: null,
            credits: null,
            state: "downloading",
            error: null,
            downloadedBytes: 0,
            totalBytes: null,
            ...overrides,
        },
    });
    return render(<UpdateChip />);
}

function fill(): HTMLElement | null {
    return document.querySelector(".tb-update-fill");
}

afterEach(() => {
    cleanup();
    setState(initial);
});

describe("UpdateChip progress", () => {
    it("fills the chip to the downloaded fraction when the size is known", () => {
        showChip({ downloadedBytes: 25, totalBytes: 100 });

        expect(fill()).not.toBeNull();
        expect(fill()!.style.transform).toBe("scaleX(0.25)");
        expect(screen.getByRole("button")).toHaveClass("tb-update-measured");
    });

    it("keeps the indeterminate pulse when the size is unknown", () => {
        showChip({ downloadedBytes: 4096, totalBytes: null });

        expect(fill()).toBeNull();
        expect(screen.getByRole("button")).not.toHaveClass("tb-update-measured");
    });

    it("shows no bar before the download starts", () => {
        showChip({ state: "available", downloadedBytes: 0, totalBytes: null });

        expect(fill()).toBeNull();
        expect(screen.getByRole("button")).not.toHaveClass("tb-update-measured");
    });
});
