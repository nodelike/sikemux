import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DatabaseProfile } from "../api";
import { blankDraft, draftOf } from "../profileForm";

const api = vi.hoisted(() => ({ save: vi.fn(), remove: vi.fn() }));
const host = vi.hoisted(() => ({ pickFile: vi.fn(), confirmDialog: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), databaseApi: api }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), ...host }));

import { ProfileForm } from "./ProfileForm";

const saved: DatabaseProfile = {
    id: "p1",
    name: "Shop",
    readOnly: false,
    agentWrites: false,
    hasPassword: true,
    engine: "postgres",
    host: "db.internal",
    port: 6543,
    database: "shop",
    user: "app",
    tls: "verify-full",
};

afterEach(cleanup);
beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    Object.values(host).forEach((mock) => mock.mockReset());
});

function renderNew(overrides: Partial<Parameters<typeof ProfileForm>[0]> = {}) {
    const props = { initial: blankDraft(), saved: null, onSaved: vi.fn(), onRemoved: vi.fn(), onCancel: vi.fn(), ...overrides };
    render(<ProfileForm {...props} />);
    return props;
}

describe("ProfileForm", () => {
    it("keeps the saved password unless a new one is typed or it is forgotten", async () => {
        api.save.mockResolvedValue(saved);
        const props = renderNew({ initial: draftOf(saved), saved });
        expect(screen.getByLabelText("Password")).toHaveAttribute("placeholder", "Saved in the Keychain");
        await act(async () => fireEvent.click(screen.getByRole("button", { name: "Save" })));
        expect(api.save).toHaveBeenLastCalledWith(draftOf(saved), undefined);
        expect(props.onSaved).toHaveBeenCalledWith(saved);

        fireEvent.click(screen.getByRole("checkbox", { name: "Forget the saved password" }));
        await act(async () => fireEvent.click(screen.getByRole("button", { name: "Save" })));
        expect(api.save).toHaveBeenLastCalledWith(draftOf(saved), "");
    });

    it("switches to SQLite and fills the file and name from the file chooser", async () => {
        host.pickFile.mockResolvedValue("/Users/me/data/app.db");
        renderNew();
        fireEvent.click(screen.getByRole("radio", { name: "SQLite" }));
        expect(screen.queryByLabelText("Host")).toBeNull();
        await act(async () => fireEvent.click(screen.getByRole("button", { name: "Choose…" })));
        expect(screen.getByLabelText("Database file")).toHaveValue("/Users/me/data/app.db");
        expect(screen.getByLabelText("Name")).toHaveValue("app.db");
    });

    it("lets agents change data only when the connection is not read only", async () => {
        api.save.mockResolvedValue(saved);
        renderNew({ initial: draftOf(saved), saved });
        const agents = screen.getByRole("checkbox", { name: /Let agents change data/ });
        fireEvent.click(agents);
        await act(async () => fireEvent.click(screen.getByRole("button", { name: "Save" })));
        expect(api.save).toHaveBeenLastCalledWith(expect.objectContaining({ agentWrites: true }), undefined);

        fireEvent.click(screen.getByRole("checkbox", { name: /Read only/ }));
        expect(screen.getByRole("checkbox", { name: /Let agents change data/ })).toBeDisabled();
        await act(async () => fireEvent.click(screen.getByRole("button", { name: "Save" })));
        expect(api.save).toHaveBeenLastCalledWith(expect.objectContaining({ readOnly: true, agentWrites: false }), undefined);
    });

    it("offers MySQL with its own default port", () => {
        renderNew();
        fireEvent.click(screen.getByRole("radio", { name: "MySQL" }));
        expect(screen.getByLabelText("Port")).toHaveAttribute("placeholder", "3306");
        expect(screen.getByLabelText("User")).toHaveAttribute("placeholder", "root");
        expect(screen.getByLabelText("Database")).toHaveAttribute("placeholder", "Optional; pick a schema once connected");
        expect(screen.getByText("Encryption")).toBeInTheDocument();
        expect(screen.getByText("Encrypt when the server offers it")).toBeInTheDocument();
    });

    it("removes a saved connection only after it is confirmed", async () => {
        const props = renderNew({ initial: draftOf(saved), saved });
        host.confirmDialog.mockResolvedValue(false);
        await act(async () => fireEvent.click(screen.getByRole("button", { name: "Remove" })));
        expect(api.remove).not.toHaveBeenCalled();
        host.confirmDialog.mockResolvedValue(true);
        api.remove.mockResolvedValue(undefined);
        await act(async () => fireEvent.click(screen.getByRole("button", { name: "Remove" })));
        expect(api.remove).toHaveBeenCalledWith("p1");
        expect(props.onRemoved).toHaveBeenCalled();
    });
});
