import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAccount } from "../account/account";
import type { AccountPhone } from "../api/account";
import { REMOTE_STATUS_EVENT, type PendingDevice, type RemoteStatus } from "../api/remote";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import { getState, setState } from "../state/store";

const notifications = vi.hoisted(() => ({ post: vi.fn(async () => {}) }));
vi.mock("../agents/agentNotifications", () => ({ postNotification: notifications.post }));
const appWindow = vi.hoisted(() => ({ requestUserAttention: vi.fn(async () => {}) }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => appWindow, UserAttentionType: { Critical: 1, Informational: 2 } }));

const { PairingPrompt } = await import("./PairingPrompt");
const { waitsLabel } = await import("./ConnectTakeover");

const PHONE = "f0e1d2c3b4a5968778695a4b3c2d1e0ff0e1d2c3b4a5968778695a4b3c2d1e0f";
const PIXEL: PendingDevice = {
    id: "join-1",
    deviceId: PHONE,
    name: "Pixel 8",
    platform: "android",
    fromAccount: true,
    expiresAt: Date.now() + 112_000,
};
const IPHONE: PendingDevice = {
    id: "join-2",
    deviceId: PHONE,
    name: "Kishore's phone",
    platform: "ios",
    fromAccount: true,
    expiresAt: Date.now() + 119_000,
};

const OTHER_PHONE = "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0";
const ACCOUNT_PIXEL: AccountPhone = { key: PHONE, name: "Pixel 8", platform: "android", createdAt: "2026-10-08T09:00:00Z" };
const ACCOUNT_IPHONE: AccountPhone = { key: OTHER_PHONE, name: "Kishore's phone", platform: "ios", createdAt: "2026-10-07T09:00:00Z" };

function status(pending: readonly PendingDevice[] = []): RemoteStatus {
    return {
        enabled: true,
        coreId: "core",
        addresses: [],
        devices: [],
        connected: [],
        pending,
        owner: "user_2abc",
        account: null,
        updateRequired: null,
        notifications: [],
    };
}

const initial = getState();
let transport: MemoryIpcTransport;

beforeEach(() => {
    setState(initial, true);
    useAccount.setState({
        account: { signedIn: true, userId: "user_2abc", email: "contact@nodelike.com", name: null, picture: null },
        phonesToAllow: [],
    });
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
    notifications.post.mockClear();
    appWindow.requestUserAttention.mockClear();
});

afterEach(() => {
    cleanup();
    resetIpcTransportForTests();
});

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
const PIXEL_ASKS = "Pixel 8 wants to connect to this computer";
const IPHONE_ASKS = "Kishore's phone wants to connect to this computer";
const takeover = (name: string) => screen.queryByRole("alertdialog", { name });

describe("PairingPrompt", () => {
    it("stays out of the way while no phone is waiting", async () => {
        transport.register("remote_status", () => status());
        render(<PairingPrompt hasFocus={() => true} />);
        await settle();
        expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    });

    it("takes over the window wherever the person is, Settings › Devices too, and goes once the request ends", async () => {
        setState({ settingsOpen: true, settingsPage: "devices" });
        transport.register("remote_status", () => status());
        const { container } = render(
            <>
                <button type="button">Behind</button>
                <PairingPrompt hasFocus={() => true} />
            </>,
        );
        await settle();

        transport.emit(REMOTE_STATUS_EVENT, status([PIXEL]));
        const asking = await screen.findByRole("alertdialog", { name: PIXEL_ASKS });
        expect(asking).toHaveTextContent("Signed in as contact@nodelike.com");
        expect(asking).toHaveTextContent(/waits 1:5\d/);
        expect(screen.getByRole("radio", { name: /Full control/ })).toHaveAttribute("aria-checked", "true");
        await waitFor(() => expect(container.inert).toBe(true));

        transport.emit(REMOTE_STATUS_EVENT, status());
        await waitFor(() => expect(takeover(PIXEL_ASKS)).not.toBeInTheDocument());
        expect(container.inert).toBe(false);
    });

    it("allows the phone with the access the person chose", async () => {
        const user = userEvent.setup();
        transport.register("remote_status", () => status([PIXEL]));
        const answer = vi.fn(() => status());
        transport.register("remote_answer_pairing", answer);
        render(<PairingPrompt hasFocus={() => true} />);

        await screen.findByRole("alertdialog", { name: PIXEL_ASKS });
        await user.click(screen.getByRole("radio", { name: /Watch only/ }));
        await user.click(screen.getByRole("button", { name: "Allow" }));

        expect(answer).toHaveBeenCalledWith({ id: "join-1", allow: true, access: "watch" }, expect.anything());
        await waitFor(() => expect(takeover(PIXEL_ASKS)).not.toBeInTheDocument());
    });

    it("answers from the keyboard, and nothing behind it hears the keys", async () => {
        const user = userEvent.setup();
        transport.register("remote_status", () => status([PIXEL]));
        const answer = vi.fn(() => status());
        transport.register("remote_answer_pairing", answer);
        const behind = vi.fn();
        window.addEventListener("keydown", behind);
        render(<PairingPrompt hasFocus={() => true} />);

        await screen.findByRole("alertdialog", { name: PIXEL_ASKS });
        await user.keyboard("{ArrowDown}");
        expect(screen.getByRole("radio", { name: /Watch only/ })).toHaveAttribute("aria-checked", "true");
        await user.keyboard("{ArrowUp}{Enter}");

        expect(answer).toHaveBeenCalledWith({ id: "join-1", allow: true, access: "full" }, expect.anything());
        expect(behind).not.toHaveBeenCalled();
        window.removeEventListener("keydown", behind);
    });

    it("declines with Escape, then asks about the next phone waiting", async () => {
        const user = userEvent.setup();
        let pending = [IPHONE, PIXEL];
        transport.register("remote_status", () => status(pending));
        const answer = vi.fn(() => {
            pending = [IPHONE];
            return status(pending);
        });
        transport.register("remote_answer_pairing", answer);
        render(<PairingPrompt hasFocus={() => true} />);

        await screen.findByRole("alertdialog", { name: PIXEL_ASKS });
        await user.keyboard("{Escape}");

        expect(answer).toHaveBeenCalledWith({ id: "join-1", allow: false, access: "full" }, expect.anything());
        expect(await screen.findByRole("alertdialog", { name: IPHONE_ASKS })).toBeInTheDocument();
        expect(screen.getByRole("radio", { name: /Full control/ })).toHaveAttribute("aria-checked", "true");
    });

    it("notifies once per request while Sikemux is in the background, and not while it is in front", async () => {
        let focused = true;
        transport.register("remote_status", () => status());
        render(<PairingPrompt hasFocus={() => focused} />);
        await settle();

        transport.emit(REMOTE_STATUS_EVENT, status([PIXEL]));
        await screen.findByRole("alertdialog", { name: PIXEL_ASKS });
        expect(notifications.post).not.toHaveBeenCalled();

        focused = false;
        transport.emit(REMOTE_STATUS_EVENT, status([PIXEL, IPHONE]));
        await waitFor(() => expect(notifications.post).toHaveBeenCalled());
        transport.emit(REMOTE_STATUS_EVENT, status([PIXEL, IPHONE]));
        await settle();

        expect(notifications.post).toHaveBeenCalledTimes(1);
        expect(notifications.post).toHaveBeenCalledWith(IPHONE_ASKS, "It is signed in to your Sikemux account. Allow or decline it in Sikemux.");
        expect(appWindow.requestUserAttention).toHaveBeenCalledTimes(1);
    });

    it("right after signing in, lets in every phone on the account at once with the access chosen", async () => {
        const user = userEvent.setup();
        transport.register("remote_status", () => status());
        const allow = vi.fn(() => status());
        transport.register("remote_allow_devices", allow);
        useAccount.setState({ phonesToAllow: [ACCOUNT_PIXEL, ACCOUNT_IPHONE] });
        render(<PairingPrompt hasFocus={() => true} />);

        const asking = await screen.findByRole("alertdialog", { name: "Let your 2 phones connect to this computer?" });
        expect(asking).toHaveTextContent("Pixel 8 · Kishore's phone");
        expect(asking).not.toHaveTextContent(/waits/);
        await user.click(screen.getByRole("radio", { name: /Watch only/ }));
        await user.click(screen.getByRole("button", { name: "Allow" }));

        expect(allow).toHaveBeenCalledWith(
            {
                devices: [
                    { id: PHONE, name: "Pixel 8", platform: "android", access: "watch" },
                    { id: OTHER_PHONE, name: "Kishore's phone", platform: "ios", access: "watch" },
                ],
            },
            expect.anything(),
        );
        await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
        expect(useAccount.getState().phonesToAllow).toEqual([]);
    });

    it("asks about a phone on the account once, though it is already waiting to join", async () => {
        const user = userEvent.setup();
        let pending: PendingDevice[] = [PIXEL];
        transport.register("remote_status", () => status(pending));
        const decline = vi.fn(() => {
            pending = [];
            return status(pending);
        });
        transport.register("remote_answer_pairing", decline);
        useAccount.setState({ phonesToAllow: [ACCOUNT_PIXEL] });
        render(<PairingPrompt hasFocus={() => true} />);

        await screen.findByRole("alertdialog", { name: "Let Pixel 8 connect to this computer?" });
        expect(takeover(PIXEL_ASKS)).not.toBeInTheDocument();
        await user.click(screen.getByRole("button", { name: "Not now" }));

        expect(decline).toHaveBeenCalledWith({ id: "join-1", allow: false, access: "full" }, expect.anything());
        await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    });

    it("counts down the minutes and seconds the phone still waits", () => {
        expect(waitsLabel(1_000 + 112_000, 1_000)).toBe("1:52");
        expect(waitsLabel(1_000, 5_000)).toBe("0:00");
    });
});
