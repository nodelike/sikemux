import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AccountInfo } from "../api/generated/wire";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import { initials } from "../lib/initials";
import { getState, setState } from "../state/store";
import { useAccount } from "./account";
import { RailAccount } from "./RailAccount";

const PICTURE = "data:image/png;base64,iVBORw0KGgo=";
const SIGNED_IN: AccountInfo = { name: "Ada Lovelace", email: "ada@example.com", pictureHash: null };

let transport: MemoryIpcTransport;
let account: AccountInfo | null;

/** The core's account calls, answering with `account` as it is when asked. */
function serve(shown: AccountInfo | null) {
    account = shown;
    transport.register("core_call", (args) => {
        const { method } = args as { method: string };
        if (method === "account") return account;
        if (method === "accountPicture") return { mime: "image/png", data: "iVBORw0KGgo=" };
        throw new Error(`unexpected core call ${method}`);
    });
}

beforeEach(() => {
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
    useAccount.setState({ account: null });
    setState({ settingsOpen: false });
});

afterEach(() => {
    cleanup();
    resetIpcTransportForTests();
});

describe("initials", () => {
    it("takes the first and last name, or the email when there is no name", () => {
        expect(initials("Ada Lovelace", "x@y.z")).toBe("AL");
        expect(initials("  ada  byron lovelace ", null)).toBe("AL");
        expect(initials("Ada", null)).toBe("A");
        expect(initials(null, "kishore@example.com")).toBe("K");
        expect(initials("", "")).toBe("");
    });
});

describe("RailAccount", () => {
    it("shows initials until a picture arrives, then the picture", async () => {
        serve({ ...SIGNED_IN, pictureHash: "abc" });
        const { container } = render(<RailAccount />);

        await vi.waitFor(() => expect(container.querySelector("img")?.getAttribute("src")).toBe(PICTURE));
        expect(screen.queryByText("AL")).toBeNull();
    });

    it("falls back to initials when the picture cannot be drawn", async () => {
        serve({ ...SIGNED_IN, pictureHash: "def" });
        const { container } = render(<RailAccount />);

        const image = await vi.waitFor(() => {
            const found = container.querySelector("img");
            if (!found) throw new Error("no picture yet");
            return found;
        });
        fireEvent.error(image);
        expect(await screen.findByText("AL")).toBeInTheDocument();
    });

    it("shows the host signed out once the account lets it go", async () => {
        serve(SIGNED_IN);
        render(<RailAccount />);

        expect(await screen.findByRole("button", { name: "Account: ada@example.com" })).toBeInTheDocument();
        account = null;
        transport.emit("core_changed", { topic: "account", project: null, paths: null });
        expect(await screen.findByRole("button", { name: "Sign in" })).toBeInTheDocument();
    });

    it("keeps Settings in reach while signed out", async () => {
        serve(null);
        render(<RailAccount />);

        expect(await screen.findByRole("button", { name: "Sign in" })).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "Settings" }));
        expect(getState().settingsOpen).toBe(true);
    });

    it("keeps Settings in reach before the account has been read", () => {
        transport.register("core_call", () => new Promise<never>(() => {}));
        render(<RailAccount />);

        expect(screen.queryByRole("button", { name: "Sign in" })).toBeNull();
        fireEvent.click(screen.getByRole("button", { name: "Settings" }));
        expect(getState().settingsOpen).toBe(true);
    });
});
