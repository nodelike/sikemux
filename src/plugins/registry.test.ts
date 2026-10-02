import { describe, expect, it } from "vitest";
import { isPluginKind, pluginIdOf } from "./kinds";
import { frontendPlugin, registerFrontendPlugin } from "./registry";

const surface = (kind: `${string}.${string}:${string}`) => ({ kind, title: "Example", icon: () => null, render: () => null });

describe("plugin kinds", () => {
    it("names a plugin's surface after the plugin", () => {
        expect(isPluginKind("sikemux.rundeck:deploy")).toBe(true);
        expect(pluginIdOf("sikemux.rundeck:deploy")).toBe("sikemux.rundeck");
        for (const kind of ["rundeck", "terminal", "sikemux.rundeck", "sikemux.rundeck/deploy", "Sikemux.rundeck:deploy", ""]) {
            expect(isPluginKind(kind)).toBe(false);
        }
    });
});

describe("frontend plugin registry", () => {
    it("refuses a surface named after another plugin", () => {
        expect(() =>
            registerFrontendPlugin({ id: "test.thief", surfaces: [surface("test.other:view")], open: () => {}, openTitle: "Open" }),
        ).toThrow();
        expect(frontendPlugin("test.thief")).toBeUndefined();
    });
});
