import { beforeEach, describe, expect, it } from "vitest";
import type { CustomCommand } from "../../commands/registry";
import * as cmd from "../commands";
import { getState, setState } from "../store";

const initial = getState();

beforeEach(() => setState(initial, true));

describe("custom command popup lifecycle", () => {
    it("assigns a fresh invocation id so an open popup terminal remounts", () => {
        const command: CustomCommand = {
            id: "logs",
            title: "Logs",
            detail: "Follow the application log",
            command: "tail -f app.log",
            placement: "popup",
            contexts: ["command"],
        };

        cmd.runCustomCommand(command);
        const first = getState().commandPopup;
        cmd.runCustomCommand(command);
        const second = getState().commandPopup;

        expect(first).toMatchObject({ title: "Logs", cwd: "" });
        expect(second).toMatchObject({ title: "Logs", cwd: "" });
        expect(second).toMatchObject({
            startup: "tail -f app.log",
            context: { sessionId: expect.any(String), sessionName: expect.any(String), sessionKind: "command" },
        });
        expect(second?.id).not.toBe(first?.id);
    });
});
