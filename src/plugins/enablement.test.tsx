import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import "./builtin";
import { SideRail } from "../components/SideRail";
import { TopBar } from "../components/TopBar";
import { Workspace } from "../components/Workspace";
import { keybindingActions, normaliseKeybindingOverrides } from "../keybindings";
import * as cmd from "../state/commands";
import { applyHydrate } from "../state/persist";
import { getState, setState } from "../state/store";

const MANIFESTS = ["sikemux.aws", "sikemux.bruno", "sikemux.rundeck", "sikemux.signoz"].map((id) => ({
    id,
    name: id,
    version: "0.1.0",
    sikemux: ">=0.4",
}));
const initial = getState();

beforeEach(() => setState({ ...initial, pluginManifests: MANIFESTS }, true));
afterEach(cleanup);

describe("switching a plugin off", () => {
    it("closes what of it is open and takes away its shortcuts, keeping the keys someone chose", () => {
        cmd.openPluginSession("sikemux.bruno:client");
        expect(Object.values(getState().sessions).some((session) => session.kind === "sikemux.bruno:client")).toBe(true);

        cmd.setPluginEnabled("sikemux.bruno", false);
        expect(getState().disabledPlugins).toEqual(["sikemux.bruno"]);
        expect(Object.values(getState().sessions).some((session) => session.kind === "sikemux.bruno:client")).toBe(false);
        expect(keybindingActions().some((action) => action.id.includes("sikemux.bruno"))).toBe(false);
        expect(normaliseKeybindingOverrides({ "plugin.run:sikemux.bruno/send": "Alt+Enter" })).toEqual({
            "plugin.run:sikemux.bruno/send": "Alt+Enter",
        });

        cmd.setPluginEnabled("sikemux.bruno", true);
        expect(getState().disabledPlugins).toEqual([]);
        expect(keybindingActions().some((action) => action.id === "plugin.run:sikemux.bruno/send")).toBe(true);
    });

    it("is remembered, and anything saved that is not a plugin id is dropped", () => {
        const project = getState().sessions[getState().activeSessionId];
        const window = getState().windows[project.activeWindowId];
        applyHydrate(
            JSON.stringify({
                version: 14,
                sessions: [project],
                windowsBySession: { [project.id]: [window] },
                sessionOrder: [project.id],
                activeSessionId: project.id,
                prefs: { disabledPlugins: ["sikemux.signoz", "not an id", 7, "sikemux.signoz"] },
                itemStates: {},
            }),
        );
        expect(getState().disabledPlugins).toEqual(["sikemux.signoz"]);
    });

    it("leaves an app that still starts and draws with every plugin off", () => {
        setState({ disabledPlugins: MANIFESTS.map((manifest) => manifest.id) });
        render(
            <>
                <TopBar />
                <SideRail />
                <Workspace />
            </>,
        );
        expect(screen.getByText("Plugins")).toBeTruthy();
        for (const name of ["AWS", "Bruno", "Rundeck", "SigNoz"]) expect(screen.queryByRole("button", { name })).toBeNull();
        expect(document.querySelector(".tb-aws-chip")).toBeNull();
    });
});
