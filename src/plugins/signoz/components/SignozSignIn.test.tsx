import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SignozStatus } from "../api";

const { inspect } = vi.hoisted(() => ({ inspect: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), signozApi: { inspect } }));

import { SignozSignIn } from "./SignozSignIn";

const status = (overrides: Partial<SignozStatus> = {}): SignozStatus => ({
    configured: false,
    url: "",
    auth: "session",
    email: "",
    keyFromEnvironment: false,
    version: null,
    ok: false,
    authFailed: false,
    message: null,
    ...overrides,
});

const found = { url: "https://logs.example.com", version: "v0.114.1", accountExists: true, orgs: [{ id: "org", name: "", password: true, sso: [] }] };

afterEach(cleanup);

beforeEach(() => {
    inspect.mockReset().mockResolvedValue(found);
});

describe("SignozSignIn", () => {
    it("starts from a remembered address and email instead of an empty form", async () => {
        await act(async () => {
            render(
                <SignozSignIn
                    status={status({ configured: true, url: "https://logs.example.com", email: "me@example.com" })}
                    onSignedIn={() => {}}
                />,
            );
        });
        expect(inspect).toHaveBeenCalledWith("https://logs.example.com", "me@example.com");
        expect(screen.queryByPlaceholderText("https://signoz.example.com")).toBeNull();
        expect(screen.getByText("logs.example.com")).toBeTruthy();
        expect(screen.getByText("SigNoz v0.114.1")).toBeTruthy();
        expect((screen.getByDisplayValue("me@example.com") as HTMLInputElement).value).toBe("me@example.com");
    });

    it("asks for the address once, then puts it away", async () => {
        render(<SignozSignIn status={status()} onSignedIn={() => {}} />);
        const address = screen.getByPlaceholderText("https://signoz.example.com");
        fireEvent.change(address, { target: { value: "https://logs.example.com/" } });
        await act(async () => {
            fireEvent.keyDown(address, { key: "Enter" });
        });
        expect(screen.queryByPlaceholderText("https://signoz.example.com")).toBeNull();
        fireEvent.click(screen.getByText("change"));
        expect(screen.getByPlaceholderText("https://signoz.example.com")).toBeTruthy();
    });
});
