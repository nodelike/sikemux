import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { getState, setState } from "../state/store";
import { UpdateChip } from "./UpdateChip";

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
    return document.querySelector(".rail-update-fill");
}

afterEach(() => {
    cleanup();
    setState(initial);
});

describe("UpdateChip progress", () => {
    it("fills the chip to the downloaded fraction when the size is known", () => {
        showChip({ downloadedBytes: 25, totalBytes: 100 });

        expect(fill()!.style.transform).toBe("scaleX(0.25)");
        expect(screen.getByRole("button")).toHaveClass("rail-update-measured");
        expect(screen.getByRole("button")).toBeDisabled();
    });

    it("keeps the indeterminate pulse when the size is unknown", () => {
        showChip({ downloadedBytes: 4096, totalBytes: null });

        expect(fill()).toBeNull();
        expect(screen.getByRole("button")).not.toHaveClass("rail-update-measured");
    });

    it("offers the update before the download starts", () => {
        showChip({ state: "available", downloadedBytes: 0, totalBytes: null });

        expect(fill()).toBeNull();
        expect(screen.getByRole("button")).toBeEnabled();
        expect(screen.getByRole("button")).toHaveTextContent("Update · v0.4.0-nightly.1");
    });

    it("renders nothing without a pending update", () => {
        render(<UpdateChip />);

        expect(screen.queryByRole("button")).toBeNull();
    });
});
