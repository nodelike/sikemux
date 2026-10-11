import { afterEach, describe, expect, it, vi } from "vitest";

const host = vi.hoisted(() => ({ copyText: vi.fn(), notify: vi.fn(), openUrl: vi.fn() }));
vi.mock("../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), ...host }));

import { issueMenu } from "./issueLinks";

const issue = { key: "ABC-12", summary: "Fix the login redirect", url: "https://acme.atlassian.net/browse/ABC-12" };

const pick = (label: string) => {
    const item = issueMenu(issue).find((entry) => entry.label === label);
    if (!item?.run) throw new Error(`no ${label}`);
    item.run();
};

afterEach(() => vi.clearAllMocks());

describe("issueMenu", () => {
    it("copies the link, the key, the key with its title, and a Markdown link, and says so", async () => {
        host.copyText.mockResolvedValue(undefined);
        pick("Copy link");
        pick("Copy key");
        pick("Copy key and title");
        pick("Copy as Markdown link");
        expect(host.copyText.mock.calls.map((call) => call[0])).toEqual([
            "https://acme.atlassian.net/browse/ABC-12",
            "ABC-12",
            "ABC-12 Fix the login redirect",
            "[ABC-12](https://acme.atlassian.net/browse/ABC-12) Fix the login redirect",
        ]);
        await Promise.resolve();
        expect(host.notify).toHaveBeenCalledWith("success", "copied the link");
    });
});
