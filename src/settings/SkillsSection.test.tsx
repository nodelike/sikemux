import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SkillsOverview } from "../api/skills";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import { SkillsSection } from "./SkillsSection";

let transport: MemoryIpcTransport;
let overview: SkillsOverview;

const skill = (name: string, description: string, path = `/data/skills/${name}`) => ({ name, description, path });

beforeEach(() => {
    overview = {
        library: "/data/skills",
        skills: [skill("deploy", "Deploys a preview")],
        own: [
            {
                agent: "Claude",
                folder: "/home/.claude/skills",
                skills: [
                    skill("deploy", "Claude's deploy", "/home/.claude/skills/deploy"),
                    skill("notes", "Takes notes", "/home/.claude/skills/notes"),
                ],
            },
        ],
        clashes: ["deploy"],
    };
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
    transport.register("skills_overview", () => structuredClone(overview));
});

afterEach(() => {
    cleanup();
    resetIpcTransportForTests();
});

describe("SkillsSection", () => {
    it("lists the Sikemux skills, flags a clash, and shows the person's own skills by agent", async () => {
        render(<SkillsSection />);
        expect(await screen.findByText("Deploys a preview", { exact: false })).toBeInTheDocument();
        expect(screen.getByText("One of your own skills has this name too.")).toBeInTheDocument();
        expect(screen.getByText("Claude · Takes notes")).toBeInTheDocument();
        expect(screen.getByText("Kept in /data/skills")).toBeInTheDocument();
    });

    it("creates a skill and shows it", async () => {
        const user = userEvent.setup();
        const create = vi.fn((args: unknown) => {
            const { name, description } = args as { name: string; description: string };
            overview.skills.push(skill(name, description));
            return skill(name, description);
        });
        transport.register("skills_create", create);
        render(<SkillsSection />);
        await user.click(await screen.findByRole("button", { name: "New skill" }));
        await user.type(screen.getByRole("textbox", { name: "Skill name" }), "review-pr");
        await user.type(screen.getByRole("textbox", { name: "What the skill does and when to use it" }), "Reviews a PR");
        await user.type(screen.getByRole("textbox", { name: "Instructions" }), "Read the diff.");
        await user.click(screen.getByRole("button", { name: "Create" }));

        expect(create).toHaveBeenCalledWith({ name: "review-pr", description: "Reviews a PR", body: "Read the diff." }, expect.anything());
        expect(await screen.findByText("Reviews a PR")).toBeInTheDocument();
    });

    it("says why a skill could not be made", async () => {
        const user = userEvent.setup();
        transport.register("skills_create", () => {
            throw new Error("'Bad' is not a skill name");
        });
        render(<SkillsSection />);
        await user.click(await screen.findByRole("button", { name: "New skill" }));
        await user.type(screen.getByRole("textbox", { name: "Skill name" }), "Bad");
        await user.type(screen.getByRole("textbox", { name: "What the skill does and when to use it" }), "x");
        await user.click(screen.getByRole("button", { name: "Create" }));
        expect(await screen.findByText(/is not a skill name/)).toBeInTheDocument();
    });

    it("moves one of the person's own skills in only after they confirm", async () => {
        const user = userEvent.setup();
        const adopt = vi.fn(() => skill("notes", "Takes notes"));
        transport.register("skills_adopt", adopt);
        render(<SkillsSection />);
        await user.click(await screen.findByRole("button", { name: "Move notes into Sikemux" }));
        expect(adopt).not.toHaveBeenCalled();
        expect(screen.getByText(/Moves it out of \/home\/.claude\/skills/)).toBeInTheDocument();
        await user.click(screen.getByRole("button", { name: "Move" }));
        expect(adopt).toHaveBeenCalledWith({ path: "/home/.claude/skills/notes" }, expect.anything());
    });

    it("says on its row why a Remove or a Move failed", async () => {
        const user = userEvent.setup();
        transport.register("skills_remove", () => {
            throw new Error("deploy is in use");
        });
        transport.register("skills_adopt", () => {
            throw new Error("A skill named notes already exists; remove it first");
        });
        render(<SkillsSection />);
        await user.click(await screen.findByRole("button", { name: "Remove deploy" }));
        expect(await screen.findByText("deploy is in use")).toBeInTheDocument();

        await user.click(screen.getByRole("button", { name: "Move notes into Sikemux" }));
        await user.click(screen.getByRole("button", { name: "Move" }));
        expect(await screen.findByText("A skill named notes already exists; remove it first")).toBeInTheDocument();
        expect(screen.queryByText("deploy is in use")).not.toBeInTheDocument();
    });

    it("removes a Sikemux skill", async () => {
        const user = userEvent.setup();
        const remove = vi.fn(() => {
            overview.skills = [];
        });
        transport.register("skills_remove", remove);
        render(<SkillsSection />);
        await user.click(await screen.findByRole("button", { name: "Remove deploy" }));
        expect(remove).toHaveBeenCalledWith({ name: "deploy" }, expect.anything());
        expect(await screen.findByText("None yet. They are kept in /data/skills")).toBeInTheDocument();
    });
});
