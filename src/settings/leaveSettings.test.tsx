import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import * as cmd from "../state/commands";
import { makeWindow } from "../state/commands/shared";
import { getState, setState } from "../state/store";
import type { Session } from "../state/types";
import { LEAVES_SETTINGS, useLeaveSettingsOnNavigation } from "./leaveSettings";

const initial = getState();

function session(id: string, activeWindowId: string): Session {
    return { id, name: id, kind: "project", cwd: `/tmp/${id}`, pinned: false, activeWindowId };
}

function Harness() {
    useLeaveSettingsOnNavigation();
    return (
        <>
            <nav {...LEAVES_SETTINGS}>
                <button type="button">current agent</button>
                <span>empty space</span>
            </nav>
            <button type="button">inside settings</button>
        </>
    );
}

beforeEach(() => {
    setState(initial, true);
    const [a, b] = [makeWindow("", "a"), makeWindow("", "b")];
    setState({
        sessions: { one: session("one", a.id) },
        sessionOrder: ["one"],
        activeSessionId: "one",
        windows: { [a.id]: a, [b.id]: b },
        windowsBySession: { one: [a.id, b.id] },
        settingsOpen: true,
    });
});

afterEach(cleanup);

describe("leaving settings", () => {
    it("closes when the workspace switches tab underneath it", () => {
        render(<Harness />);

        cmd.selectWindowId(getState().windowsBySession.one[1]);

        expect(getState().settingsOpen).toBe(false);
    });

    it("closes on a button in a navigation surface even when nothing moves", () => {
        render(<Harness />);

        fireEvent.click(screen.getByRole("button", { name: "current agent" }));

        expect(getState().settingsOpen).toBe(false);
    });

    it("stays open for clicks inside settings and on empty rail space", () => {
        render(<Harness />);

        fireEvent.click(screen.getByRole("button", { name: "inside settings" }));
        fireEvent.click(screen.getByText("empty space"));

        expect(getState().settingsOpen).toBe(true);
    });
});
