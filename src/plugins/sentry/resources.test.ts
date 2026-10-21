import { describe, expect, it } from "vitest";
import { resourceKind } from "../../plugin-api/identity";
import { SENTRY_PLUGIN } from "./kinds";
import { sentry, sentryIssuesR } from "./resources";

describe("the Sentry plugin's cached reads", () => {
    it("keeps its status where a call that finds the person signed out looks for it", () => {
        expect(sentry.statusR.kind).toBe(resourceKind(SENTRY_PLUGIN.id, "status"));
    });

    it("names every read under the plugin, so the pane's Refresh reaches them all", () => {
        expect(sentry.owns(sentryIssuesR.kind)).toBe(true);
        expect(sentry.owns("sentry.issues")).toBe(false);
    });
});
