import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DatabaseProfileDraft, DatabaseProfileForm } from "../types.generated";

const api = vi.hoisted(() => ({ save: vi.fn(), remove: vi.fn() }));
const host = vi.hoisted(() => ({ pickFile: vi.fn(), confirmDialog: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), databaseApi: api }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), ...host }));

import { ProfileForm } from "./ProfileForm";

const shop: DatabaseProfileDraft = {
    id: "p1",
    name: "Shop",
    engine: "postgres",
    host: "db.internal",
    port: "6543",
    database: "shop",
    user: "app",
    tls: "verify-full",
    path: "",
    readOnly: false,
    agentWrites: false,
};

const choices: Omit<DatabaseProfileForm, "draft" | "hasPassword"> = {
    engines: [
        {
            engine: "postgres",
            label: "PostgreSQL",
            defaultPort: 5432,
            nameHint: "Production",
            userHint: "postgres",
            databaseHint: "Same as the user name",
            pathHint: "",
        },
        {
            engine: "mysql",
            label: "MySQL",
            defaultPort: 3306,
            nameHint: "Production",
            userHint: "root",
            databaseHint: "Optional; pick a schema once connected",
            pathHint: "",
        },
        {
            engine: "sqlite",
            label: "SQLite",
            defaultPort: null,
            nameHint: "Local app data",
            userHint: "",
            databaseHint: "",
            pathHint: "~/data/app.db",
        },
    ],
    tlsModes: [
        { value: "prefer", label: "Prefer", detail: "Encrypt when the server offers it" },
        { value: "verify-full", label: "Verify", detail: "Always encrypt, trusted certificate only" },
    ],
};

const savedForm: DatabaseProfileForm = { ...choices, draft: shop, hasPassword: true };
const newForm: DatabaseProfileForm = {
    ...choices,
    draft: { ...shop, id: null, name: "", host: "localhost", port: "", database: "", user: "", tls: "prefer" },
    hasPassword: false,
};

afterEach(cleanup);
beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    Object.values(host).forEach((mock) => mock.mockReset());
});

function renderNew(overrides: Partial<Parameters<typeof ProfileForm>[0]> = {}) {
    const props = { form: newForm, onSaved: vi.fn(), onRemoved: vi.fn(), onCancel: vi.fn(), ...overrides };
    render(<ProfileForm {...props} />);
    return props;
}

describe("ProfileForm", () => {
    it("keeps the saved password unless a new one is typed or it is forgotten", async () => {
        api.save.mockResolvedValue(undefined);
        const props = renderNew({ form: savedForm });
        expect(screen.getByLabelText("Password")).toHaveAttribute("placeholder", "Saved in the Keychain");
        await act(async () => fireEvent.click(screen.getByRole("button", { name: "Save" })));
        expect(api.save).toHaveBeenLastCalledWith(shop, undefined);
        expect(props.onSaved).toHaveBeenCalled();

        fireEvent.click(screen.getByRole("checkbox", { name: "Forget the saved password" }));
        await act(async () => fireEvent.click(screen.getByRole("button", { name: "Save" })));
        expect(api.save).toHaveBeenLastCalledWith(shop, "");
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
        api.save.mockResolvedValue(undefined);
        renderNew({ form: savedForm });
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
        const props = renderNew({ form: savedForm });
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
