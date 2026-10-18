import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import { useToasts } from "../state/toast";
import { GLOBAL_SKILL_ADDED_EVENT, GlobalSkillsBridge } from "./GlobalSkillsBridge";

let transport: MemoryIpcTransport;

beforeEach(() => {
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
    useToasts.setState({ toasts: [] });
});

afterEach(() => {
    cleanup();
    resetIpcTransportForTests();
});

describe("GlobalSkillsBridge", () => {
    it("offers to move a skill an agent saved in its own folder, and moves it when asked", async () => {
        const adopt = vi.fn(() => ({ name: "notes", description: "", path: "/data/skills/notes" }));
        transport.register("skills_adopt", adopt);
        render(<GlobalSkillsBridge />);
        await act(async () => {});

        await act(async () => transport.emit(GLOBAL_SKILL_ADDED_EVENT, { agent: "Claude", name: "notes", path: "/home/.claude/skills/notes" }));
        const [toast] = useToasts.getState().toasts;
        expect(toast.text).toContain("Claude saved the skill notes in its own folder");
        expect(toast.action?.label).toBe("Move into Sikemux");

        await act(async () => toast.action?.run(toast.id));
        expect(adopt).toHaveBeenCalledWith({ path: "/home/.claude/skills/notes" }, expect.anything());
        expect(useToasts.getState().toasts.some((each) => each.text.startsWith("notes is in Sikemux now"))).toBe(true);
    });
});
