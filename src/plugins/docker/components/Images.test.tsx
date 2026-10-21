import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Image } from "../api";

const storage = vi.hoisted(() => ({ pruneImages: vi.fn(), removeImage: vi.fn(), runImage: vi.fn() }));
vi.mock("../storage", async (importOriginal) => ({ ...(await importOriginal<object>()), ...storage }));

import { Images } from "./Images";

const images: Image[] = [
    { id: "9d1e4f7a2c30", tags: ["shop-web:dev"], created: 1_792_000_000, size: 600, containers: 1, dangling: false },
    { id: "e4a7d2c9b1f8", tags: [], created: 1_791_000_000, size: 400, containers: 0, dangling: true },
];

afterEach(cleanup);

describe("the images list", () => {
    it("filters to unused or untagged images, and offers to clear the untagged ones", () => {
        render(<Images images={images} find="" />);
        const table = screen.getByRole("table", { name: "Images" });
        expect(within(table).getAllByRole("row")).toHaveLength(3);
        fireEvent.click(screen.getByRole("button", { name: /^Untagged/ }));
        expect(within(table).getAllByRole("row")).toHaveLength(2);
        expect(within(table).getByText("untagged")).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: /Remove untagged/ }));
        expect(storage.pruneImages).toHaveBeenCalled();
    });

    it("runs or removes an image from its row", () => {
        render(<Images images={images} find="shop" />);
        const row = screen.getByRole("row", { name: "shop-web:dev" });
        fireEvent.click(within(row).getByRole("button", { name: "Run" }));
        expect(storage.runImage).toHaveBeenCalledWith(images[0]);
        fireEvent.click(within(row).getByRole("button", { name: "Remove" }));
        expect(storage.removeImage).toHaveBeenCalledWith(images[0]);
    });
});
