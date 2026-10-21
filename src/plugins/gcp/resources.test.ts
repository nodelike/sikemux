import { describe, expect, it } from "vitest";
import { resourceKind } from "../../plugin-api/identity";
import { GCP_PLUGIN } from "./kinds";
import { gcp, gcpStatusR, runServicesR } from "./resources";

describe("the Google Cloud plugin's cached reads", () => {
    it("keeps its status where a call that finds the person signed out looks for it", () => {
        expect(gcpStatusR.kind).toBe(resourceKind(GCP_PLUGIN.id, "status"));
    });

    it("names every read under the plugin, so the pane's Refresh reaches them all", () => {
        expect(gcp.owns(runServicesR.kind)).toBe(true);
        expect(gcp.owns("gcp.run.services")).toBe(false);
    });
});
