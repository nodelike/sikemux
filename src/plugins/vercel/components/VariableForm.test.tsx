import { vercel } from "../resources";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { VercelVariable } from "../api";

const api = vi.hoisted(() => ({ setEnv: vi.fn(), removeEnv: vi.fn(), env: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), vercelApi: api }));
const host = vi.hoisted(() => ({ confirmDialog: vi.fn(), notify: vi.fn() }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), ...host }));

import { Environment } from "./Settings";
import { changeOf, problemOf, VariableForm } from "./VariableForm";

const databaseUrl: VercelVariable = { id: "4EV2iWMf", key: "DATABASE_URL", target: ["production"], type: "encrypted", gitBranch: null, updatedAt: 1 };

afterEach(() => {
    cleanup();
    vercel.refresh();
});
beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    Object.values(host).forEach((mock) => mock.mockReset());
    api.env.mockResolvedValue([databaseUrl]);
    api.setEnv.mockResolvedValue(databaseUrl);
    api.removeEnv.mockResolvedValue(undefined);
    host.confirmDialog.mockResolvedValue(true);
});

const draft = { key: "API_URL", value: "x", targets: ["preview" as const], branch: "", sensitive: false };

describe("an environment variable's form", () => {
    it("says what's wrong before anything is sent", () => {
        expect(problemOf({ ...draft, key: "1BAD" }, false)).toMatch(/doesn't start with a digit/);
        expect(problemOf({ ...draft, value: "" }, false)).toBe("Enter a value.");
        expect(problemOf({ ...draft, value: "" }, true)).toBeNull();
        expect(problemOf({ ...draft, targets: [] }, false)).toBe("Pick at least one environment.");
        expect(problemOf({ ...draft, targets: ["production", "preview"], branch: "feat/x" }, false)).toMatch(/Preview alone/);
        expect(problemOf({ ...draft, targets: ["development"], sensitive: true }, false)).toMatch(/can't be used in Development/);
    });

    it("keeps the value of a variable being changed unless a new one is typed", () => {
        expect(changeOf({ ...draft, value: "" }, databaseUrl)).toEqual({ id: "4EV2iWMf", key: "API_URL", targets: ["preview"] });
        expect(changeOf({ ...draft, sensitive: true, branch: " feat/x " }, null)).toEqual({
            kind: "sensitive",
            key: "API_URL",
            value: "x",
            targets: ["preview"],
            branch: "feat/x",
        });
    });

    it("adds a variable with its value hidden as it is typed", async () => {
        const onDone = vi.fn();
        render(<VariableForm team="team_acme" project="web" editing={null} onDone={onDone} />);
        fireEvent.change(screen.getByRole("textbox", { name: "Key" }), { target: { value: "STRIPE_KEY" } });
        const value = screen.getByLabelText("Value");
        expect(value).toHaveAttribute("type", "password");
        fireEvent.change(value, { target: { value: "sk_live_1" } });
        fireEvent.click(screen.getByRole("button", { name: "Add" }));
        await waitFor(() =>
            expect(api.setEnv).toHaveBeenCalledWith("team_acme", "web", {
                kind: "encrypted",
                key: "STRIPE_KEY",
                value: "sk_live_1",
                targets: ["production", "preview", "development"],
            }),
        );
        expect(onDone).toHaveBeenCalled();
    });
});

describe("the environment variables list", () => {
    it("edits a variable in place, and removes one after asking", async () => {
        render(<Environment active team="team_acme" project="web" find="" />);
        const row = await screen.findByRole("row", { name: "DATABASE_URL" });
        fireEvent.click(within(row).getByRole("button", { name: "Edit" }));
        expect(screen.getByRole("form", { name: "Edit DATABASE_URL" })).toBeInTheDocument();
        fireEvent.click(within(row).getByRole("button", { name: "Remove" }));
        await waitFor(() => expect(api.removeEnv).toHaveBeenCalledWith("team_acme", "web", "4EV2iWMf"));
        expect(host.confirmDialog).toHaveBeenCalledWith(expect.objectContaining({ title: "Remove DATABASE_URL?", destructive: true }));
    });
});
