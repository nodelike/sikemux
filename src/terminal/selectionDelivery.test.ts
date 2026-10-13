import { describe, expect, it } from "vitest";
import type { Window } from "../state/types";
import { terminalName, terminalSelectionDelivery } from "./selectionDelivery";

describe("terminalSelectionDelivery", () => {
    it("names the terminal and its folder, then fences the selection", () => {
        expect(terminalSelectionDelivery("npm test\nFAIL a.test.ts\n", "zsh", "/repo").text).toBe(
            'From the terminal "zsh" in /repo:\n\n```\nnpm test\nFAIL a.test.ts\n```\n',
        );
    });

    it("still reads when neither is known", () => {
        expect(terminalSelectionDelivery("ls", null, undefined).text).toBe("From a terminal:\n\n```\nls\n```\n");
    });
});

describe("terminalName", () => {
    const win: Window = {
        id: "w",
        name: "Shells",
        role: "term",
        activePaneId: "p",
        root: { type: "pane", id: "p", cwd: "/repo", kind: "terminal", title: "server" },
    };

    it("prefers the pane's own title over its window's", () => {
        const context = { sessionId: "s", sessionName: "repo", sessionKind: "project" as const, windowId: "w", paneId: "p" };
        expect(terminalName({ windows: { w: win }, terminalTitles: {} }, context)).toBe("server");
        expect(terminalName({ windows: { w: win }, terminalTitles: {} }, { ...context, paneId: "gone" })).toBe("Shells");
        expect(terminalName({ windows: {}, terminalTitles: {} }, context)).toBeNull();
    });

    it("goes by the name the person gave the tab, then by the shell's title", () => {
        const context = { sessionId: "s", sessionName: "repo", sessionKind: "project" as const, windowId: "w", paneId: "p" };
        expect(terminalName({ windows: { w: win }, terminalTitles: { p: "~/repo" } }, context)).toBe("~/repo");
        expect(terminalName({ windows: { w: { ...win, customName: "dev server" } }, terminalTitles: { p: "~/repo" } }, context)).toBe("dev server");
    });
});
