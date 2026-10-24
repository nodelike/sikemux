import { describe, expect, it, beforeEach } from "vitest";
import {
    addFilter,
    removeFilter,
    scopeOf,
    setLive,
    showSection,
    showService,
    signalOf,
    signozSettings,
    togglePin,
    viewOf,
    zoomTo,
    type SignozSettings,
} from "./state";
import { answerPreferencesForTests } from "../../plugin-api/testPreferences";

beforeEach(answerPreferencesForTests);

describe("signozSettings", () => {
    it("falls back to usable settings whatever was saved", () => {
        const saved = {
            minutes: 7,
            serviceSort: "vibes",
            environment: 3,
            serviceByProject: { "/repo": "api", "/bad": 3 },
        } as unknown as SignozSettings;
        signozSettings.update(() => saved);
        expect(signozSettings.get()).toEqual({
            minutes: 15,
            environment: null,
            serviceSort: "errors",
            serviceByProject: { "/repo": "api" },
            dashboardVariables: {},
            pins: [],
        });
    });
});

describe("filters", () => {
    it("replaces a filter on the same attribute and operator instead of stacking it", () => {
        addFilter("pane-filters", { key: "path", op: "equals", value: "/a" });
        addFilter("pane-filters", { key: "path", op: "not-equals", value: "/health" });
        addFilter("pane-filters", { key: "path", op: "equals", value: "/b" });
        expect(viewOf("pane-filters").filters).toEqual([
            { key: "path", op: "not-equals", value: "/health" },
            { key: "path", op: "equals", value: "/b" },
        ]);
        removeFilter("pane-filters", 0);
        expect(viewOf("pane-filters").filters).toEqual([{ key: "path", op: "equals", value: "/b" }]);
    });
});

describe("scopeOf", () => {
    it("reads the last N minutes while live, and a fixed window once held", () => {
        const settings = { minutes: 15, environment: "production" };
        expect(scopeOf(viewOf("pane-live"), settings)).toMatchObject({ minutes: 15, environment: "production" });

        setLive("pane-held", false);
        const held = scopeOf(viewOf("pane-held"), settings);
        expect(held.minutes).toBeUndefined();
        expect(held.end! - held.start!).toBe(15 * 60_000);
    });

    it("holds a moment picked out of a chart until live is back on", () => {
        zoomTo("pane-zoom", { start: 1_000, end: 5_000 });
        expect(scopeOf(viewOf("pane-zoom"), { minutes: 15, environment: null })).toMatchObject({ start: 1_000, end: 5_000 });
        setLive("pane-zoom", true);
        expect(scopeOf(viewOf("pane-zoom"), { minutes: 15, environment: null })).toMatchObject({ minutes: 15 });
    });
});

describe("service pages", () => {
    it("scope the view to the service only while its page is open", () => {
        const settings = { minutes: 15, environment: null };
        showService("pane-service", "api-gateway", "logs");
        expect(scopeOf(viewOf("pane-service"), settings).service).toBe("api-gateway");
        expect(signalOf(viewOf("pane-service"))).toBe("logs");
        showSection("pane-service", "logs");
        expect(scopeOf(viewOf("pane-service"), settings).service).toBeUndefined();
    });

    it("read no signal on the overview", () => {
        showService("pane-overview", "api-gateway");
        expect(signalOf(viewOf("pane-overview"))).toBeNull();
    });
});

describe("pins", () => {
    it("toggle, and drop anything saved that is not a pin", () => {
        signozSettings.update((settings) => ({ ...settings, pins: ["service:api", "nonsense", 3] as unknown as SignozSettings["pins"] }));
        expect(signozSettings.get().pins).toEqual(["service:api"]);
        togglePin("dashboard:d1");
        togglePin("service:api");
        expect(signozSettings.get().pins).toEqual(["dashboard:d1"]);
    });
});
