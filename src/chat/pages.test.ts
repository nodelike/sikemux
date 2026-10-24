import { describe, expect, it } from "vitest";
import { readPageMessage } from "./pages";

describe("readPageMessage", () => {
    it("takes a size report and a web link", () => {
        expect(readPageMessage({ jsonrpc: "2.0", method: "ui/notifications/size-changed", params: { height: 230 } })).toEqual({
            kind: "height",
            height: 230,
        });
        expect(readPageMessage({ jsonrpc: "2.0", id: "l1", method: "ui/open-link", params: { url: "https://a.test/x" } })).toEqual({
            kind: "link",
            url: "https://a.test/x",
        });
    });

    it("takes a wheel the page could not use", () => {
        expect(readPageMessage({ jsonrpc: "2.0", method: "sikemux/wheel", params: { deltaY: -48 } })).toEqual({ kind: "wheel", deltaY: -48 });
        expect(readPageMessage({ jsonrpc: "2.0", method: "sikemux/wheel", params: { deltaY: "far" } })).toBeNull();
    });

    it("refuses links that are not to the web and anything malformed", () => {
        expect(readPageMessage({ jsonrpc: "2.0", method: "ui/open-link", params: { url: "file:///etc/passwd" } })).toBeNull();
        expect(readPageMessage({ jsonrpc: "2.0", method: "ui/notifications/size-changed", params: { height: -1 } })).toBeNull();
        expect(readPageMessage({ method: "ui/notifications/size-changed", params: { height: 10 } })).toBeNull();
        expect(readPageMessage("hello")).toBeNull();
    });
});
