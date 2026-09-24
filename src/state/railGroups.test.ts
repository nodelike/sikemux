import { describe, expect, it } from "vitest";
import type { PluginManifest } from "../api/plugins";
import { railGroupOf } from "./railGroups";

const rundeck: PluginManifest = { id: "sikemux.rundeck", name: "Rundeck", version: "0.1.0", sikemux: ">=0.4" };

describe("railGroupOf", () => {
    it("puts every plugin's sessions under plugins", () => {
        expect(railGroupOf("sikemux.rundeck:deploy", [rundeck])).toBe("plugins");
    });

    it("leaves out sessions of a plugin this build does not have", () => {
        expect(railGroupOf("sikemux.rundeck:deploy", [])).toBeNull();
    });

    it("keeps core sessions where they were", () => {
        expect(railGroupOf("project", [])).toBe("project");
    });
});
