import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AcpChat } from "../api/acp";
import { resetResourcesForTests } from "../state/resources";
import { getState, setState } from "../state/store";
import type { Agent, ProviderProfile } from "../state/types";
import { switchAccount } from "./accounts";

const mocks = vi.hoisted(() => ({
    list: vi.fn(),
    switchAccount: vi.fn(async () => {}),
    account: vi.fn(),
}));

vi.mock("../api/acp", () => ({ acpApi: { list: mocks.list, switchAccount: mocks.switchAccount } }));
vi.mock("../api/agents", () => ({ agentApi: { account: mocks.account } }));

const initial = getState();

const home: ProviderProfile = { id: "home", name: "Home", provider: "claude", accent: "#000", configPath: "~/.claude-home" };
const elsewhere: ProviderProfile = { id: "elsewhere", name: "Elsewhere", provider: "claude", accent: "#000", configPath: "~/.claude-elsewhere" };

function agent(id: string, fields: Partial<Agent> = {}): Agent {
    return { id, type: "claude", title: id, startup: "claude", launchState: "live", ...fields };
}

function chat(agentId: string): AcpChat {
    return {
        agentId,
        provider: "claude",
        cwd: "/repo",
        sessionId: "s",
        state: "ready",
        running: true,
        pendingPermissions: [],
        startedBy: null,
        launcher: null,
        permissionMode: "bypass",
        model: null,
        effort: null,
    };
}

beforeEach(() => {
    vi.clearAllMocks();
    resetResourcesForTests();
    setState(initial, true);
    mocks.account.mockImplementation(async (_type: string, configPath?: string) => ({
        signedIn: true,
        sessions: configPath === elsewhere.configPath ? "/elsewhere/projects" : "/shared/projects",
    }));
});

describe("switchAccount", () => {
    it("starts new chats on the account, moves open ones there and wakes sleeping ones there", async () => {
        setState({
            providerProfiles: [...initial.providerProfiles, home, elsewhere],
            agents: {
                running: agent("running"),
                asleep: agent("asleep", { launchState: "dormant" }),
                already: agent("already", { profileId: "home" }),
                apart: agent("apart", { profileId: "elsewhere" }),
                terminal: agent("terminal", { ptyId: 7 }),
                codex: agent("codex", { type: "codex" }),
            },
        });
        mocks.list.mockResolvedValue([chat("running"), chat("apart")]);

        await switchAccount("claude", home);

        expect(getState().selectedProviderProfileIds.claude).toBe("home");
        expect(mocks.switchAccount).toHaveBeenCalledTimes(1);
        expect(mocks.switchAccount).toHaveBeenCalledWith("running", "claude", { id: "home", label: "Home", configPath: "~/.claude-home" });
        await vi.waitFor(() => expect(getState().agents.running.profileId).toBe("home"));
        expect(getState().agents.asleep.profileId).toBe("home");
        expect(getState().agents.apart.profileId).toBe("elsewhere");
        expect(getState().agents.terminal.profileId).toBeUndefined();
        expect(getState().agents.codex.profileId).toBeUndefined();
    });
});
