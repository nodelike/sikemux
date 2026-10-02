import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { REMOTE_STATUS_EVENT, type RemoteStatus } from "../api/remote";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import { DevicesPage, seenLabel } from "./DevicesPage";

const CORE = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2";
const PHONE = "f0e1d2c3b4a5968778695a4b3c2d1e0ff0e1d2c3b4a5968778695a4b3c2d1e0f";

function status(overrides: Partial<RemoteStatus> = {}): RemoteStatus {
    return {
        enabled: true,
        coreId: CORE,
        addresses: ["192.168.0.2:53786"],
        devices: [],
        connected: [],
        pairing: null,
        pending: [],
        ...overrides,
    };
}

let transport: MemoryIpcTransport;

beforeEach(() => {
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
});

afterEach(() => {
    cleanup();
    resetIpcTransportForTests();
    vi.useRealTimers();
});

describe("DevicesPage", () => {
    it("turns remote access on and shows the way to pair once it is", async () => {
        const user = userEvent.setup();
        transport.register("remote_status", () => status({ enabled: false }));
        const setEnabled = vi.fn(() => status());
        transport.register("remote_set_enabled", setEnabled);
        render(<DevicesPage />);

        expect(await screen.findByText("off")).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "Pair a device" })).toBeDisabled();
        await user.click(screen.getByRole("switch", { name: "Allow paired devices" }));

        expect(setEnabled).toHaveBeenCalledWith({ enabled: true }, expect.anything());
        await waitFor(() => expect(screen.getByRole("button", { name: "Pair a device" })).toBeEnabled());
        expect(screen.getByText(CORE.slice(0, 8))).toBeInTheDocument();
    });

    it("shows the code, then lets the person allow the device that typed it", async () => {
        const user = userEvent.setup();
        const expiresAt = Date.now() + 4 * 60_000;
        transport.register("remote_status", () => status());
        transport.register("remote_open_pairing", () =>
            status({ pairing: { code: "482913", expiresAt, link: "sikemux://pair?core=core&code=482913" } }),
        );
        const answer = vi.fn(() =>
            status({
                devices: [{ id: PHONE, name: "Kishore's phone", platform: "ios", access: "watch", pairedAt: 1, lastSeen: null }],
            }),
        );
        transport.register("remote_answer_pairing", answer);
        render(<DevicesPage />);

        await user.click(await screen.findByRole("button", { name: "Pair a device" }));
        expect(await screen.findByText("482 913")).toBeInTheDocument();
        expect(screen.getByRole("img", { name: "Pairing QR code" })).toBeInTheDocument();
        expect(screen.getByText(/Expires in 4:00/)).toBeInTheDocument();

        transport.emit(REMOTE_STATUS_EVENT, status({ pending: [{ id: "request-1", deviceId: PHONE, name: "Kishore's phone", platform: "ios" }] }));
        expect(await screen.findByText("Kishore's phone wants to pair")).toBeInTheDocument();
        expect(screen.getByText(PHONE.slice(0, 8))).toBeInTheDocument();

        await user.click(screen.getByRole("button", { name: /access for this device/ }));
        await user.click(await screen.findByRole("option", { name: /Watch and approve/ }));
        await user.click(screen.getByRole("button", { name: "Allow" }));

        expect(answer).toHaveBeenCalledWith({ id: "request-1", allow: true, access: "watch" }, expect.anything());
        expect(await screen.findByText("iOS · never connected")).toBeInTheDocument();
    });

    it("revokes a paired device", async () => {
        const user = userEvent.setup();
        const device = { id: PHONE, name: "Phone", platform: "android", access: "full" as const, pairedAt: 1, lastSeen: Date.now() };
        transport.register("remote_status", () => status({ devices: [device], connected: [PHONE] }));
        const revoke = vi.fn(() => status());
        transport.register("remote_revoke_device", revoke);
        render(<DevicesPage />);

        expect(await screen.findByText("Android · connected now")).toBeInTheDocument();
        await user.click(screen.getByRole("button", { name: /Revoke/ }));

        expect(revoke).toHaveBeenCalledWith({ id: PHONE }, expect.anything());
        expect(await screen.findByText(/No devices yet/)).toBeInTheDocument();
    });
});

describe("seenLabel", () => {
    it("says when a device was last seen in the largest whole unit", () => {
        const now = Date.UTC(2026, 9, 2, 12);
        expect(seenLabel(null, now)).toBe("never connected");
        expect(seenLabel(now - 20_000, now)).toBe("seen just now");
        expect(seenLabel(now - 5 * 60_000, now)).toBe("seen 5 min ago");
        expect(seenLabel(now - 3 * 3_600_000, now)).toBe("seen 3 h ago");
    });
});
