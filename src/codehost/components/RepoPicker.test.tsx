import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
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

beforeEach(() => {
    invalidate(() => true);
    api.myRepos.mockReset().mockResolvedValue([listing("me/alpha"), listing("me/beta"), listing("me/gamma")]);
    hostSettings(TEST_HOST).update((settings) => ({ ...settings, pinned: [] }));
});

afterEach(cleanup);

describe("RepoPicker", () => {
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

    it("does nothing on Enter or the arrows with nothing listed", async () => {
        api.myRepos.mockResolvedValue([]);
        const { input, onPick } = open(null, "empty-id");
        await waitFor(() => expect(screen.getByText("no matches")).toBeTruthy());
        fireEvent.keyDown(input, { key: "ArrowDown" });
        fireEvent.keyDown(input, { key: "ArrowUp" });
        fireEvent.keyDown(input, { key: "Enter" });
        expect(onPick).not.toHaveBeenCalled();
    });
});
