import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Stack } from "../api";
import { SHOP_DB, SHOP_WEB } from "../testing";

const compose = vi.hoisted(() => ({ runCompose: vi.fn(), actOnStack: vi.fn(() => Promise.resolve()) }));
vi.mock("../compose", () => compose);

import { Stacks } from "./Stacks";

const shop: Stack = {
    project: "shop",
    workingDir: "/Users/me/shop",
    configFiles: ["/Users/me/shop/compose.yaml"],
    services: [
        { name: "db", containers: [SHOP_DB] },
        { name: "web", containers: [SHOP_WEB] },
    ],
    running: 1,
    total: 2,
};

afterEach(cleanup);

describe("Compose stacks", () => {
    it("shows each service's containers, and opens one", () => {
        const onOpen = vi.fn();
        render(<Stacks stacks={[shop]} find="" onOpen={onOpen} />);
        const card = screen.getByRole("region", { name: "shop" });
        expect(within(card).getByText("1 of 2 running")).toBeInTheDocument();
        expect(within(card).getByText(/compose\.yaml/)).toBeInTheDocument();
        fireEvent.click(within(card).getByRole("button", { name: /shop-db-1/ }));
        expect(onOpen).toHaveBeenCalledWith("shop-db-1");
    });

    it("brings a partly running stack up, and acts on the whole of it", () => {
        render(<Stacks stacks={[shop]} find="" onOpen={vi.fn()} />);
        const actions = screen.getByRole("group", { name: "shop actions" });
        fireEvent.click(within(actions).getByRole("button", { name: "Up" }));
        expect(compose.runCompose).toHaveBeenCalledWith(shop, "Up", "up", "-d");
        fireEvent.click(within(actions).getByRole("button", { name: "Down" }));
        expect(compose.actOnStack).toHaveBeenCalledWith(shop, "down");
    });

    it("says how stacks come to be when there are none, and when none match", () => {
        const { rerender } = render(<Stacks stacks={[]} find="" onOpen={vi.fn()} />);
        expect(screen.getByText(/docker compose up show here/)).toBeInTheDocument();
        rerender(<Stacks stacks={[shop]} find="billing" onOpen={vi.fn()} />);
        expect(screen.getByText("No stacks match.")).toBeInTheDocument();
    });
});
