import { describe, expect, it } from "vitest";
import { resourceKind } from "../../plugin-api/identity";
import { AZURE_PLUGIN } from "./kinds";
import { azure, azureVmsR } from "./resources";

describe("the Azure plugin's cached reads", () => {
    it("keeps its status where a call that finds the person signed out looks for it", () => {
        expect(azure.statusR.kind).toBe(resourceKind(AZURE_PLUGIN.id, "status"));
    });

    it("names every read under the plugin, so the pane's Refresh reaches them all", () => {
        expect(azure.owns(azureVmsR.kind)).toBe(true);
        expect(azure.owns("azure.vms")).toBe(false);
    });
});
