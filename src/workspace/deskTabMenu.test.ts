import { describe, expect, it, vi } from "vitest";
import type { DeskItem } from "../state/desks";
import { deskTabMenu } from "./deskTabMenu";

const labels = (items: ReturnType<typeof deskTabMenu>) => items.map((item) => (item.sep ? "—" : item.label));
const choose = (items: ReturnType<typeof deskTabMenu>, label: string) => items.find((item) => item.label === label)?.run?.();

function actions() {
    return { copy: vi.fn(), reveal: vi.fn(), close: vi.fn() };
}

const file: DeskItem = { key: "file:/Users/me/shop/docs/database.html", kind: "file", path: "/Users/me/shop/docs/database.html" };

describe("deskTabMenu", () => {
    it("copies a file's absolute path or its path within the project", () => {
        const run = actions();
        const menu = deskTabMenu(file, "/Users/me/shop", run);
        expect(labels(menu)).toEqual(["Copy Path", "Copy Relative Path", "—", expect.stringMatching(/^Reveal in /), "—", "Close"]);
        choose(menu, "Copy Path");
        expect(run.copy).toHaveBeenLastCalledWith("/Users/me/shop/docs/database.html", "path");
        choose(menu, "Copy Relative Path");
        expect(run.copy).toHaveBeenLastCalledWith("docs/database.html", "relative path");
    });

    it("falls back to the file's name when it is outside the project or there is none", () => {
        const run = actions();
        choose(deskTabMenu(file, "/Users/me/other", run), "Copy Relative Path");
        expect(run.copy).toHaveBeenLastCalledWith("database.html", "relative path");
        choose(deskTabMenu(file, null, run), "Copy Relative Path");
        expect(run.copy).toHaveBeenLastCalledWith("database.html", "relative path");
    });

    it("copies a page's link, except for a blank tab", () => {
        const run = actions();
        const page = { key: "b1", kind: "browser", tab: { id: "t1", url: "https://example.com/docs", title: "Docs" } } as unknown as DeskItem;
        const menu = deskTabMenu(page, null, run);
        expect(labels(menu)).toEqual(["Copy Link", "—", "Close"]);
        choose(menu, "Copy Link");
        expect(run.copy).toHaveBeenCalledWith("https://example.com/docs", "link");
        const blank = { ...page, tab: { id: "t2", url: "about:blank", title: "" } } as unknown as DeskItem;
        expect(deskTabMenu(blank, null, run)[0].disabled).toBe(true);
    });

    it("offers a terminal tab only Close", () => {
        const terminal = { key: "term:1", kind: "terminal", terminal: { id: "1", label: "zsh" } } as unknown as DeskItem;
        expect(labels(deskTabMenu(terminal, null, actions()))).toEqual(["Close"]);
    });
});
