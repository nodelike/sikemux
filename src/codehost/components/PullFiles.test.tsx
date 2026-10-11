import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ChangedFile } from "../api";

const api = vi.hoisted(() => ({ pullFiles: vi.fn() }));
vi.mock("../../agents/SendToAgentMenu", () => ({
    SendToAgentMenu: ({ delivery }: { delivery: () => { text?: string } }) => <pre data-testid="send-menu">{delivery().text}</pre>,
}));

import { invalidate } from "../../plugin-api/resources";
import { InHost, registerTestHost, TEST_HOST } from "../testHost";
import { PullFiles } from "./PullFiles";

const host = registerTestHost(api);
const repo = { provider: TEST_HOST, owner: "nodelike", name: "sikemux" };

const file = (path: string, status: string, extra: Partial<ChangedFile> = {}): ChangedFile => ({
    path,
    status,
    additions: 1,
    deletions: 2,
    patch: "@@ -1,1 +1,1 @@\n-a\n+b",
    ...extra,
});

const show = (number = 5) =>
    render(
        <InHost host={host}>
            <PullFiles repo={repo} pull={{ number, title: "Fix", url: `https://github.com/o/r/pull/${number}` }} cwd="/repo" active />
        </InHost>,
    );

beforeEach(() => {
    invalidate(() => true);
    api.pullFiles.mockReset();
    vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(2_000);
    vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(1_000);
});

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

describe("PullFiles", () => {
    it("sends the line right-clicked in a diff to an agent", async () => {
        api.pullFiles.mockResolvedValue([file("a.ts", "modified")]);
        show(4);
        fireEvent.contextMenu(await screen.findByText("b"));
        fireEvent.click(screen.getByRole("menuitem", { name: "Send Line to Agent…" }));
        expect(screen.getByTestId("send-menu").textContent).toBe(
            'From pull request #4 "Fix" (https://github.com/o/r/pull/4), a.ts, new line 1:\n\n```diff\n+b\n```\n',
        );
    });
});
