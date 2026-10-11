import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { REMOTE_STATUS_EVENT, type RemoteStatus } from "../api/remote";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import { useAccount } from "../account/account";
import { getState, setState } from "../state/store";
import { DevicesPage, PHONE_URL, seenLabel } from "./DevicesPage";

const CORE = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2";
const PHONE = "f0e1d2c3b4a5968778695a4b3c2d1e0ff0e1d2c3b4a5968778695a4b3c2d1e0f";

function status(overrides: Partial<RemoteStatus> = {}): RemoteStatus {
    return {
        enabled: true,
        coreId: CORE,
        addresses: ["192.168.0.2:53786"],
        devices: [],
        connected: [],
        pending: [],
        owner: null,
        account: null,
        updateRequired: null,
        notifications: [],
        ...overrides,
    };
}

let transport: MemoryIpcTransport;

const SIGNED_OUT = { signedIn: false, userId: null, email: null, name: null, picture: null };
const SIGNED_IN = { signedIn: true, userId: "user_2abc", email: "me@example.com", name: null, picture: null };
let account: typeof SIGNED_OUT | typeof SIGNED_IN;

beforeEach(() => {
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
    account = SIGNED_OUT;
    transport.register("account_status", () => account);
    transport.register("account_refresh_profile", () => account);
    useAccount.setState({ account: null });
});

afterEach(() => {
    cleanup();
    resetIpcTransportForTests();
    vi.useRealTimers();
});

describe("DevicesPage", () => {
    it("asks a signed-out computer to sign in and keeps remote access off", async () => {
        transport.register("remote_status", () => status({ enabled: false }));
        render(<DevicesPage />);

        expect(await screen.findByRole("heading", { name: "Sign in to your Sikemux account" })).toBeInTheDocument();
        expect(screen.getByText("Sign in to use Sikemux on your phone")).toBeInTheDocument();
        expect(screen.getByRole("switch", { name: "Remote access" })).toBeDisabled();
        expect(screen.queryByRole("img", { name: `QR code for ${PHONE_URL}` })).not.toBeInTheDocument();
        expect(screen.queryByText("Phones")).not.toBeInTheDocument();
    });

    it("shows the account, the switch and the way to get the phone app while no phone is allowed", async () => {
        const user = userEvent.setup();
        account = SIGNED_IN;
        transport.register("remote_status", () => status({ enabled: false, owner: "user_2abc" }));
        const setEnabled = vi.fn(() => status({ owner: "user_2abc" }));
        transport.register("remote_set_enabled", setEnabled);
        render(<DevicesPage />);

        expect(await screen.findByText("me@example.com")).toBeInTheDocument();
        expect(screen.getByText("Signed in to Sikemux")).toBeInTheDocument();
        expect(screen.getByRole("img", { name: `QR code for ${PHONE_URL}` })).toBeInTheDocument();
        expect(screen.getByText("Click Allow here when it asks")).toBeInTheDocument();
        expect(screen.getByText("None yet. Waiting for your phone.")).toBeInTheDocument();

        await user.click(screen.getByRole("switch", { name: "Remote access" }));
        expect(setEnabled).toHaveBeenCalledWith({ enabled: true }, expect.anything());
    });

    it("lists the phones once one is allowed, in place of the way to get the app", async () => {
        const user = userEvent.setup();
        account = SIGNED_IN;
        const device = { id: PHONE, name: "Phone", platform: "android", access: "full" as const, pairedAt: 1, lastSeen: Date.now() };
        transport.register("remote_status", () => status({ owner: "user_2abc", devices: [device], connected: [PHONE] }));
        const revoke = vi.fn(() => status({ owner: "user_2abc" }));
        transport.register("remote_revoke_device", revoke);
        render(<DevicesPage />);

        expect(await screen.findByText("Android · connected now")).toBeInTheDocument();
        expect(screen.queryByRole("img", { name: `QR code for ${PHONE_URL}` })).not.toBeInTheDocument();
        await user.click(screen.getByRole("button", { name: /Remove/ }));

        expect(revoke).toHaveBeenCalledWith({ id: PHONE }, expect.anything());
        expect(await screen.findByText("None yet. Waiting for your phone.")).toBeInTheDocument();
        expect(screen.getByRole("img", { name: `QR code for ${PHONE_URL}` })).toBeInTheDocument();
    });

    it("drops a phone the account revoked as soon as the host hears", async () => {
        account = SIGNED_IN;
        const phone = { id: PHONE, name: "Pixel", platform: "ios", access: "full" as const, pairedAt: 1, lastSeen: null };
        transport.register("remote_status", () => status({ owner: "user_2abc", devices: [phone] }));
        render(<DevicesPage />);

        expect(await screen.findByText("Pixel")).toBeInTheDocument();
        transport.emit(REMOTE_STATUS_EVENT, status({ owner: "user_2abc", devices: [] }));
        expect(await screen.findByText("None yet. Waiting for your phone.")).toBeInTheDocument();
    });
});

describe("DevicesPage and the account", () => {
    const initial = getState();
    beforeEach(() => setState(initial, true));

    it("shows when the connection to the account is down", async () => {
        account = SIGNED_IN;
        transport.register("remote_status", () => status({ owner: "user_2abc", account: { state: "offline", reason: null, since: 1 } }));
        render(<DevicesPage />);

        expect(await screen.findByText("Can't reach your account, retrying")).toBeInTheDocument();
        transport.emit(REMOTE_STATUS_EVENT, status({ owner: "user_2abc", account: { state: "live", reason: null, since: 2 } }));
        expect(await screen.findByText("Signed in to Sikemux")).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "Delete account…" })).toBeInTheDocument();
    });

    it("signs out and says why when the account lets this host go", async () => {
        account = SIGNED_IN;
        transport.register("remote_status", () => status({ owner: "user_2abc", account: { state: "live", reason: null, since: 1 } }));
        render(<DevicesPage />);

        expect(await screen.findByText("me@example.com")).toBeInTheDocument();
        account = SIGNED_OUT;
        transport.emit(REMOTE_STATUS_EVENT, status({ account: { state: "removed", reason: "account_deleted", since: 3 } }));

        expect(await screen.findByText("Your account was deleted. Phones already allowed stay allowed.")).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "Sign in" })).toBeInTheDocument();
    });

    it("signs in through the browser, lands on Settings › Devices, then offers to sign out", async () => {
        const user = userEvent.setup();
        transport.register("remote_status", () => status({ enabled: false }));
        let finish: (value: unknown) => void = () => {};
        transport.register("account_sign_in", () => new Promise((resolve) => (finish = resolve)));
        transport.register("account_sign_out", () => SIGNED_OUT);
        render(<DevicesPage />);

        await user.click(await screen.findByRole("button", { name: "Sign in" }));
        expect(await screen.findByText("Finish signing in in your browser")).toBeInTheDocument();
        setState({ settingsOpen: false, settingsPage: "general" });
        finish(SIGNED_IN);

        expect(await screen.findByText("me@example.com")).toBeInTheDocument();
        expect(getState().settingsOpen).toBe(true);
        expect(getState().settingsPage).toBe("devices");
        await user.click(screen.getByRole("button", { name: "Sign out" }));
        expect(await screen.findByRole("button", { name: "Sign in" })).toBeInTheDocument();
    });

    it("lets the person give up on a sign-in left open in the browser", async () => {
        const user = userEvent.setup();
        transport.register("remote_status", () => status());
        let fail: (error: unknown) => void = () => {};
        transport.register("account_sign_in", () => new Promise((_, reject) => (fail = reject)));
        const cancel = vi.fn(() => {
            fail(new Error("sign-in was cancelled"));
        });
        transport.register("account_cancel_sign_in", cancel);
        render(<DevicesPage />);

        await user.click(await screen.findByRole("button", { name: "Sign in" }));
        await user.click(await screen.findByRole("button", { name: "Cancel" }));

        expect(cancel).toHaveBeenCalled();
        expect(await screen.findByRole("button", { name: "Sign in" })).toBeInTheDocument();
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

describe("DevicesPage and notifications", () => {
    const phone = { id: PHONE, name: "Pixel", platform: "android", access: "watch" as const, pairedAt: 1, lastSeen: null };

    it("says beside each phone whether its notifications reach it", async () => {
        transport.register("remote_status", () => status({ devices: [phone], notifications: [{ deviceId: PHONE, state: "notReaching", since: 1 }] }));
        render(<DevicesPage />);
        expect(await screen.findByText("notifications aren't reaching it")).toHaveClass("device-warning");
    });

    it("says nothing of notifications for a phone that never asked", async () => {
        transport.register("remote_status", () => status({ devices: [phone] }));
        render(<DevicesPage />);
        expect(await screen.findByText("Pixel")).toBeInTheDocument();
        expect(screen.queryByText(/notifications/)).not.toBeInTheDocument();
    });
});
