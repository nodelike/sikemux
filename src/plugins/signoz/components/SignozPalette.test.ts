import { describe, expect, it } from "vitest";
import { viewOf } from "../state";
import { paletteItems } from "./SignozPalette";

describe("paletteItems", () => {
    it("offers a pasted trace id first, and opens it", () => {
        const items = paletteItems("16DB92378D1E0E36039248E129D148F5", ["api-gateway"]);
        expect(items[0].label).toBe("Open trace 16db92378d1e0e36039248e129d148f5");
        items[0].run("pane-palette-trace");
        expect(viewOf("pane-palette-trace").trace).toBe("16db92378d1e0e36039248e129d148f5");
    });

    it("finds a service by part of its name and picks it", () => {
        const items = paletteItems("gatew", ["reel-worker", "api-gateway"]);
        expect(items[0].label).toBe("api-gateway");
        items[0].run("pane-palette-service");
        expect(viewOf("pane-palette-service")).toMatchObject({ section: "services", service: "api-gateway", serviceTab: "overview" });
    });

    it("lists actions before services when nothing is typed", () => {
        const labels = paletteItems("", ["api-gateway"]).map((item) => item.label);
        expect(labels.indexOf("Search traces")).toBeLessThan(labels.indexOf("api-gateway"));
        expect(labels).not.toContain(expect.stringMatching(/^Open trace/));
    });

    it("opens a dashboard by its title", () => {
        const items = paletteItems("render", ["reel-worker"], [{ id: "d1", title: "Render farm" }]);
        expect(items[0].label).toBe("Render farm");
        items[0].run("pane-palette-dashboard");
        expect(viewOf("pane-palette-dashboard")).toMatchObject({ section: "dashboards", dashboard: "d1" });
    });
});
