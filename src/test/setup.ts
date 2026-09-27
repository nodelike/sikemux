import "@testing-library/jest-dom/vitest";
import { vi } from "vitest";
import type { MarkdownRequest } from "../api/markdown";

// jsdom implements no scrolling, so components that keep a selection in view
// would throw here rather than in a browser.
if (!Element.prototype.scrollIntoView) {
    Element.prototype.scrollIntoView = () => {};
}

// jsdom lays nothing out, so nothing here ever reports a resize — but panes
// that watch their content for one still need the constructor to exist.
if (!("ResizeObserver" in globalThis)) {
    globalThis.ResizeObserver = class {
        observe() {}
        unobserve() {}
        disconnect() {}
    } as unknown as typeof ResizeObserver;
}

vi.mock("../api/markdown", async () => {
    const { parseWithFixtures } = await import("./markdownDouble");
    return { markdownApi: { parse: async (request: MarkdownRequest) => parseWithFixtures(request) } };
});
