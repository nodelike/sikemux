import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Volume } from "../api";

const storage = vi.hoisted(() => ({ pruneVolumes: vi.fn(), removeVolume: vi.fn() }));
vi.mock("../storage", async (importOriginal) => ({ ...(await importOriginal<object>()), ...storage }));

import { Volumes } from "./Volumes";

const volumes: Volume[] = [
    {
        name: "shop_pgdata",
        driver: "local",
        created: "2026-09-01T00:00:00Z",
        size: 2048,
        usedBy: ["shop-db-1"],
        anonymous: false,
        composeProject: "shop",
    },
    { name: "3fa91c0e7b2d9a8f", driver: "local", created: null, size: null, usedBy: [], anonymous: true, composeProject: null },
];

afterEach(cleanup);

describe("the volumes list", () => {
    it("keeps a volume in use from being removed, and opens the container using it", () => {
        const onOpen = vi.fn();
        render(<Volumes volumes={volumes} find="" onOpen={onOpen} />);
        const row = screen.getByRole("row", { name: "shop_pgdata" });
        expect(within(row).getByRole("button", { name: "Remove" })).toBeDisabled();
        fireEvent.click(within(row).getByRole("button", { name: "shop-db-1" }));
        expect(onOpen).toHaveBeenCalledWith("shop-db-1");
    });

    it("removes an unused one, and clears unused anonymous ones together", () => {
        render(<Volumes volumes={volumes} find="" onOpen={vi.fn()} />);
        const loose = screen.getByRole("row", { name: "3fa91c0e7b2d9a8f" });
        expect(within(loose).getByText("3fa91c0e7b2d")).toBeInTheDocument();
        fireEvent.click(within(loose).getByRole("button", { name: "Remove" }));
        expect(storage.removeVolume).toHaveBeenCalledWith(volumes[1]);
        fireEvent.click(screen.getByRole("button", { name: "Remove unused anonymous (1)" }));
        expect(storage.pruneVolumes).toHaveBeenCalled();
    });
});
