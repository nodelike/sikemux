import { beforeEach, describe, expect, it } from "vitest";
import * as cmd from "../commands";
import { getState, setState } from "../store";
import type { Session, SessionKind } from "../types";

const initial = getState();

beforeEach(() => setState(initial, true));

function session(id: string, kind: SessionKind): Session {
    return {
        id,
        name: id,
        kind,
        cwd: `/${id}`,
        pinned: false,
        activeWindowId: "",
    };
}

describe("session ordering", () => {
    it("reorders projects without moving the slots occupied by other session kinds", () => {
        const sessions = {
            alpha: session("alpha", "project"),
            ssh: session("ssh", "ssh"),
            beta: session("beta", "project"),
            command: session("command", "command"),
            gamma: session("gamma", "project"),
        };
        setState({ sessions, sessionOrder: ["alpha", "ssh", "beta", "command", "gamma"] });

        cmd.reorderSession("gamma", "alpha", "before");

        expect(getState().sessionOrder).toEqual(["gamma", "ssh", "alpha", "command", "beta"]);
    });

    it("supports placing a project after another project", () => {
        const sessions = {
            alpha: session("alpha", "project"),
            beta: session("beta", "project"),
            gamma: session("gamma", "project"),
        };
        setState({ sessions, sessionOrder: ["alpha", "beta", "gamma"] });

        cmd.reorderSession("alpha", "beta", "after");

        expect(getState().sessionOrder).toEqual(["beta", "alpha", "gamma"]);
    });

    it("ignores invalid and cross-kind reorder requests", () => {
        const sessions = {
            alpha: session("alpha", "project"),
            beta: session("beta", "project"),
            ssh: session("ssh", "ssh"),
        };
        setState({ sessions, sessionOrder: ["alpha", "ssh", "beta"] });

        cmd.reorderSession("alpha", "ssh", "after");
        cmd.reorderSession("missing", "beta", "before");
        cmd.reorderSession("alpha", "alpha", "after");

        expect(getState().sessionOrder).toEqual(["alpha", "ssh", "beta"]);
    });
});
