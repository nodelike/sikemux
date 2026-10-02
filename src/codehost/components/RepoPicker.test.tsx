import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RepoListing } from "../api";

const api = vi.hoisted(() => ({ myRepos: vi.fn() }));

import { invalidate } from "../../plugin-api/resources";
import { AccountProvider } from "../registry";
import { hostSettings } from "../state";
import { InHost, registerTestHost, TEST_HOST } from "../testHost";
import { RepoPicker } from "./RepoPicker";

const host = registerTestHost(api);

const listing = (slug: string): RepoListing => {
    const [owner = "", name = ""] = slug.split("/");
    return { owner, name, slug, private: false, archived: true, defaultBranch: "main", pushedAt: null, url: "" };
};

function open(current: { owner: string; name: string } | null = null, account: string | null = null) {
    const onPick = vi.fn();
    const onClose = vi.fn();
    render(
        <InHost host={host}>
            <AccountProvider value={account}>
                <RepoPicker current={current && { provider: TEST_HOST, ...current }} onPick={onPick} onClose={onClose} />
            </AccountProvider>
        </InHost>,
    );
    return { onPick, onClose, input: screen.getByPlaceholderText(/Search your repositories/) };
}

const names = () => Array.from(document.querySelectorAll(".picker-item .picker-name")).map((node) => node.textContent);
const selectedName = () => document.querySelector(".picker-item.sel .picker-name")?.textContent;

beforeEach(() => {
    invalidate(() => true);
    api.myRepos.mockReset().mockResolvedValue([listing("me/alpha"), listing("me/beta"), listing("me/gamma")]);
    hostSettings(TEST_HOST).update((settings) => ({ ...settings, pinned: [] }));
});

afterEach(cleanup);

describe("RepoPicker", () => {
    it("lists the account's repositories and marks the one already open", async () => {
        open({ owner: "me", name: "beta" }, "ada-id");
        await waitFor(() => expect(names()).toEqual(["me/alpha", "me/beta", "me/gamma"]));
        expect(api.myRepos).toHaveBeenCalledWith("ada-id");
        expect(screen.getByText("Your repositories")).toBeTruthy();
        const beta = screen.getByText("me/beta").closest("button")!;
        expect(within(beta).getByText("open")).toBeTruthy();
        expect(within(screen.getByText("me/alpha").closest("button")!).getByText("archived")).toBeTruthy();
    });

    it("says it is loading until the list arrives", () => {
        api.myRepos.mockReturnValue(new Promise(() => {}));
        open(null, "slow-id");
        expect(screen.getByText("loading…")).toBeTruthy();
    });

    it("offers to open a typed owner/repo that is not in the list", async () => {
        const { input, onPick, onClose } = open();
        await waitFor(() => expect(names()).toHaveLength(3));
        await userEvent.type(input, "someone/else");
        expect(names()[0]).toBe("someone/else");
        expect(screen.getByText("open it")).toBeTruthy();
        await userEvent.keyboard("{Enter}");
        expect(onPick).toHaveBeenCalledWith({ provider: TEST_HOST, owner: "someone", name: "else" });
        expect(onClose).toHaveBeenCalled();
    });

    it("does not offer a typed repository twice when it is already listed", async () => {
        const { input } = open();
        await waitFor(() => expect(names()).toHaveLength(3));
        await userEvent.type(input, "me/beta");
        expect(names()).toEqual(["me/beta"]);
        expect(screen.queryByText("open it")).toBeNull();
    });

    it("moves the selection with the arrows and Tab, wrapping at both ends", async () => {
        const { input, onPick } = open();
        await waitFor(() => expect(names()).toHaveLength(3));
        expect(selectedName()).toBe("me/alpha");
        fireEvent.keyDown(input, { key: "ArrowUp" });
        expect(selectedName()).toBe("me/gamma");
        fireEvent.keyDown(input, { key: "ArrowDown" });
        expect(selectedName()).toBe("me/alpha");
        fireEvent.keyDown(input, { key: "Tab" });
        expect(selectedName()).toBe("me/beta");
        fireEvent.keyDown(input, { key: "Tab", shiftKey: true });
        expect(selectedName()).toBe("me/alpha");
        fireEvent.keyDown(input, { key: "ArrowDown" });
        fireEvent.keyDown(input, { key: "Enter" });
        expect(onPick).toHaveBeenCalledWith({ provider: TEST_HOST, owner: "me", name: "beta" });
    });

    it("does nothing on Enter or the arrows with nothing listed", async () => {
        api.myRepos.mockResolvedValue([]);
        const { input, onPick } = open(null, "empty-id");
        await waitFor(() => expect(screen.getByText("no matches")).toBeTruthy());
        fireEvent.keyDown(input, { key: "ArrowDown" });
        fireEvent.keyDown(input, { key: "ArrowUp" });
        fireEvent.keyDown(input, { key: "Enter" });
        expect(onPick).not.toHaveBeenCalled();
    });

    it("picks a repository clicked", async () => {
        const { onPick, onClose } = open();
        await userEvent.click(await screen.findByText("me/gamma"));
        expect(onPick).toHaveBeenCalledWith({ provider: TEST_HOST, owner: "me", name: "gamma" });
        expect(onClose).toHaveBeenCalled();
    });

    it("closes on Escape and on a click outside, but not on a click inside", async () => {
        const { input, onClose } = open();
        fireEvent.keyDown(input, { key: "Escape" });
        expect(onClose).toHaveBeenCalledTimes(1);
        fireEvent.mouseDown(screen.getByRole("dialog"));
        expect(onClose).toHaveBeenCalledTimes(1);
        fireEvent.mouseDown(document.querySelector(".picker-backdrop")!);
        expect(onClose).toHaveBeenCalledTimes(2);
    });

    it("pins the selected repository to the top, and unpins it", async () => {
        const { input } = open();
        await waitFor(() => expect(names()).toHaveLength(3));
        fireEvent.keyDown(input, { key: "ArrowDown" });
        const pin = screen.getByRole("button", { name: "Pin me/beta" });
        expect(pin.getAttribute("aria-pressed")).toBe("false");
        await userEvent.click(pin);
        expect(hostSettings(TEST_HOST).get().pinned).toEqual(["me/beta"]);
        await waitFor(() => expect(names()[0]).toBe("me/beta"));
        expect(screen.getByText("Pinned")).toBeTruthy();
        fireEvent.keyDown(input, { key: "ArrowUp" });
        await userEvent.click(screen.getByRole("button", { name: "Unpin me/beta" }));
        expect(hostSettings(TEST_HOST).get().pinned).toEqual([]);
    });

    it("has no pin for a typed repository that is not listed", async () => {
        const { input } = open();
        await userEvent.type(input, "some/where");
        expect(selectedName()).toBe("some/where");
        expect(screen.queryByRole("button", { name: /Pin/ })).toBeNull();
    });
});
