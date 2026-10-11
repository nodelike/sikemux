import { beforeEach, describe, expect, it, vi } from "vitest";
import { deliverToAgent, receiveForAgent } from "../../agents/agentInbox";
import * as cmd from "../commands";
import { acceptDialog, resetDialogsForTests, useDialogs } from "../dialog";
import { agentWindowId } from "../selectors";
import { getState, setState } from "../store";

const initial = getState();

beforeEach(() => {
    setState(initial, true);
    resetDialogsForTests();
});

function newChatIn(cwd: string): string {
    cmd.createProjectSession(cwd);
    cmd.addAgent("claude");
    const state = getState();
    const winId = state.sessions[state.activeSessionId].activeWindowId;
    return Object.values(state.agents).find((agent) => agentWindowId(state, agent.id) === winId)!.id;
}

describe("deliveries to an agent that closes", () => {
    it("are dropped when its tab closes", () => {
        const agentId = newChatIn("/work/app");
        deliverToAgent(agentId, { text: "never read" });

        cmd.closeWindowById(agentWindowId(getState(), agentId)!);

        expect(getState().agents[agentId]).toBeUndefined();
        const receiver = vi.fn();
        receiveForAgent(agentId, receiver)();
        expect(receiver).not.toHaveBeenCalled();
    });

    it("are dropped when its project closes", async () => {
        cmd.createProjectSession("/work/app");
        const agentId = newChatIn("/work/site");
        deliverToAgent(agentId, { paths: ["/work/site/a.ts"] });

        cmd.closeSession(getState().activeSessionId);
        acceptDialog(useDialogs.getState().dialog!.id);
        await Promise.resolve();

        expect(getState().agents[agentId]).toBeUndefined();
        const receiver = vi.fn();
        receiveForAgent(agentId, receiver)();
        expect(receiver).not.toHaveBeenCalled();
    });
});
