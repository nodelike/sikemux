import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { REMOTE_STATUS_EVENT, type RemoteStatus } from "../api/remote";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import { UpdateRequiredPrompt } from "./UpdateRequiredPrompt";

vi.mock("@tauri-apps/plugin-process", () => ({ exit: vi.fn(), relaunch: vi.fn() }));

function status(updateRequired: RemoteStatus["updateRequired"]): RemoteStatus {
    return {
        enabled: true,
        coreId: "core",
        addresses: [],
        devices: [],
        connected: [],
        pending: [],
        owner: null,
        account: null,
        updateRequired,
        notifications: [],
        requireApproval: false,
    };
}

let transport: MemoryIpcTransport;

beforeEach(() => {
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
    transport.register("update_check", () => null);
});

afterEach(() => {
    cleanup();
    resetIpcTransportForTests();
});

describe("UpdateRequiredPrompt", () => {
    it("stays out of the way while the build is new enough", async () => {
        transport.registerCoreRequest("remoteStatus", () => ({ status: status(null) }));
        render(<UpdateRequiredPrompt />);
        await new Promise((resolve) => setTimeout(resolve, 0));
        expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    });

    it("covers the app once the core says this build is too old, offering only to update or quit", async () => {
        transport.registerCoreRequest("remoteStatus", () => ({ status: status(null) }));
        render(<UpdateRequiredPrompt />);
        await new Promise((resolve) => setTimeout(resolve, 0));
        transport.emit(REMOTE_STATUS_EVENT, status({ current: "0.5.0-nightly.1", minimum: "0.5.0-nightly.4" }));

        expect(await screen.findByRole("alertdialog", { name: "Update Sikemux" })).toBeInTheDocument();
        expect(screen.getByText(/This version, 0.5.0-nightly.1, is older/)).toHaveTextContent("update to 0.5.0-nightly.4 or later");
        expect(screen.getByRole("button", { name: "Quit Sikemux" })).toBeEnabled();
        expect(screen.getByRole("button", { name: "Check for updates" })).toBeEnabled();
        expect(screen.queryByRole("button", { name: /close/i })).not.toBeInTheDocument();
    });
});
