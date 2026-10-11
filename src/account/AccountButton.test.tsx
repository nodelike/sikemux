import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ACCOUNT_CHANGED_EVENT, type AccountStatus } from "../api/account";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import { initials, useAccount } from "./account";
import { AccountButton } from "./AccountButton";

const PICTURE = "data:image/png;base64,iVBORw0KGgo=";
const SIGNED_OUT: AccountStatus = { signedIn: false, userId: null, email: null, name: null, picture: null };
const SIGNED_IN: AccountStatus = { signedIn: true, userId: "user_1", email: "ada@example.com", name: "Ada Lovelace", picture: null };

let transport: MemoryIpcTransport;

function serve(cached: AccountStatus, refreshed: AccountStatus = cached) {
    transport.register("account_status", () => cached);
    const refresh = vi.fn(() => refreshed);
    transport.register("account_refresh_profile", refresh);
    return refresh;
}

beforeEach(() => {
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
    useAccount.setState({ account: null });
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

describe("AccountButton", () => {
    it("shows initials until a picture arrives, then the picture", async () => {
        serve(SIGNED_IN, { ...SIGNED_IN, picture: PICTURE });
        const { container } = render(<AccountButton />);

        const button = await screen.findByRole("button", { name: "Account: ada@example.com" });
        await vi.waitFor(() => expect(container.querySelector("img")?.getAttribute("src")).toBe(PICTURE));
        expect(button.textContent).toBe("");
    });

    it("falls back to initials when the picture cannot be drawn", async () => {
        serve({ ...SIGNED_IN, picture: PICTURE });
        const { container } = render(<AccountButton />);

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
        render(<AccountButton />);

        expect(await screen.findByRole("button", { name: "Account: ada@example.com" })).toBeInTheDocument();
        transport.emit(ACCOUNT_CHANGED_EVENT, SIGNED_OUT);
        expect(await screen.findByRole("button", { name: "Account: not signed in" })).toBeInTheDocument();
    });
});
