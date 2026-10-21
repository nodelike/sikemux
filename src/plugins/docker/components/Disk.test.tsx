import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Disk as DiskUsage } from "../api";

const storage = vi.hoisted(() => ({ pruneImages: vi.fn(), pruneVolumes: vi.fn() }));
vi.mock("../storage", async (importOriginal) => ({ ...(await importOriginal<object>()), ...storage }));
const host = vi.hoisted(() => ({ runInTerminal: vi.fn(() => Promise.resolve()) }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), ...host }));

import { Disk } from "./Disk";

const MB = 1024 * 1024;
const usage: DiskUsage = {
    images: { count: 5, active: 4, size: 400 * MB, reclaimable: 100 * MB },
    containers: { count: 5, active: 4, size: 20 * MB, reclaimable: 0 },
    volumes: { count: 3, active: 2, size: 80 * MB, reclaimable: 0 },
    buildCache: { count: 41, active: 0, size: 500 * MB, reclaimable: 500 * MB },
};

afterEach(cleanup);

describe("disk use", () => {
    it("adds up what Docker uses and what could be freed", () => {
        render(<Disk disk={usage} onSection={vi.fn()} />);
        expect(screen.getByText("1000 MB")).toBeInTheDocument();
        expect(screen.getByText(/600 MB could be freed/)).toBeInTheDocument();
        expect(screen.getByRole("meter", { name: "Images that could be freed" })).toHaveAttribute("aria-valuenow", "25");
    });

    it("clears what can be cleared, and goes to the section to look first", () => {
        const onSection = vi.fn();
        render(<Disk disk={usage} onSection={onSection} />);
        const volumes = screen.getByRole("region", { name: "Volumes" });
        expect(within(volumes).getByRole("button", { name: "Remove unused anonymous" })).toBeDisabled();
        fireEvent.click(within(screen.getByRole("region", { name: "Images" })).getByRole("button", { name: "Remove untagged" }));
        expect(storage.pruneImages).toHaveBeenCalled();
        fireEvent.click(within(screen.getByRole("region", { name: "Build cache" })).getByRole("button", { name: "Clear in a terminal" }));
        expect(host.runInTerminal).toHaveBeenCalledWith("Clear the build cache", ["docker", "builder", "prune"]);
        fireEvent.click(within(volumes).getByRole("button", { name: "Show volumes" }));
        expect(onSection).toHaveBeenCalledWith("volumes");
    });
});
