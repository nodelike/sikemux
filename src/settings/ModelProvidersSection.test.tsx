import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ModelProvider } from "../api/modelProviders";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import { ModelProvidersSection } from "./ModelProvidersSection";

let transport: MemoryIpcTransport;
let providers: ModelProvider[];

function row(label: string): HTMLElement {
    const element = screen.getByText(label).closest<HTMLElement>(".settings-row");
    if (!element) throw new Error(`no row for ${label}`);
    return element;
}

beforeEach(() => {
    providers = [
        { id: "openrouter", label: "OpenRouter", keysUrl: "https://openrouter.ai/settings/keys", connected: false },
        { id: "baseten", label: "Baseten", keysUrl: "https://app.baseten.co/settings/api_keys", connected: false },
    ];
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
    transport.register("model_providers", () => providers.map((provider) => ({ ...provider })));
});

afterEach(() => {
    cleanup();
    resetIpcTransportForTests();
});

describe("ModelProvidersSection", () => {
    it("saves a pasted key and shows the provider as connected", async () => {
        const user = userEvent.setup();
        const connect = vi.fn((args: unknown) => {
            const { id } = args as { id: string };
            providers = providers.map((provider) => (provider.id === id ? { ...provider, connected: true } : provider));
        });
        transport.register("model_provider_connect", connect);
        render(<ModelProvidersSection />);

        await screen.findByText("Baseten");
        await user.click(within(row("Baseten")).getByRole("button", { name: "Add key" }));
        await user.type(screen.getByLabelText("Baseten API key"), "bt-secret");
        await user.click(screen.getByRole("button", { name: "Connect" }));

        expect(connect).toHaveBeenCalledWith({ id: "baseten", key: "bt-secret" }, expect.anything());
        expect(await within(row("Baseten")).findByRole("button", { name: "Disconnect" })).toBeInTheDocument();
        expect(screen.getByText("1 connected")).toBeInTheDocument();
        expect(within(row("OpenRouter")).getByRole("button", { name: "Add key" })).toBeInTheDocument();
    });

    it("keeps the key field open and says why when the provider refuses the key", async () => {
        const user = userEvent.setup();
        transport.register("model_provider_connect", () => {
            throw new Error("Baseten did not accept that key");
        });
        render(<ModelProvidersSection />);

        await screen.findByText("Baseten");
        await user.click(within(row("Baseten")).getByRole("button", { name: "Add key" }));
        await user.type(screen.getByLabelText("Baseten API key"), "wrong{Enter}");

        expect(await screen.findByText("Baseten did not accept that key")).toBeInTheDocument();
        expect(screen.getByLabelText("Baseten API key")).toHaveValue("wrong");
    });

    it("forgets a connected provider's key", async () => {
        const user = userEvent.setup();
        providers[0].connected = true;
        const disconnect = vi.fn(() => {
            providers[0].connected = false;
        });
        transport.register("model_provider_disconnect", disconnect);
        render(<ModelProvidersSection />);

        await screen.findByText("OpenRouter");
        await user.click(within(row("OpenRouter")).getByRole("button", { name: "Disconnect" }));

        expect(disconnect).toHaveBeenCalledWith({ id: "openrouter" }, expect.anything());
        expect(await within(row("OpenRouter")).findByRole("button", { name: "Add key" })).toBeInTheDocument();
    });
});
