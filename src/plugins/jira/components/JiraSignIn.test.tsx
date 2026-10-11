import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { JiraStatus } from "../api";

const { signIn, signInWithBrowser } = vi.hoisted(() => ({ signIn: vi.fn(), signInWithBrowser: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), jiraApi: { signIn, signInWithBrowser } }));

import { JiraSignIn } from "./JiraSignIn";

const signedIn: JiraStatus = {
    configured: true,
    sites: [{ host: "acme.atlassian.net", displayName: "Me", default: true }],
    ok: true,
    authFailed: false,
    message: null,
    browserSignIn: false,
};
const signedOutWithBrowser: JiraStatus = { ...signedIn, configured: false, sites: [], ok: false, browserSignIn: true };

afterEach(cleanup);
beforeEach(() => {
    signIn.mockReset();
    signInWithBrowser.mockReset();
});

function fill(site: string, email: string, token: string) {
    fireEvent.change(screen.getByPlaceholderText("your-team.atlassian.net"), { target: { value: site } });
    fireEvent.change(screen.getByPlaceholderText("you@example.com"), { target: { value: email } });
    fireEvent.change(screen.getByPlaceholderText("ATATT…"), { target: { value: token } });
}

describe("JiraSignIn", () => {
    it("signs in with the site, email and token once all three are given", async () => {
        signIn.mockResolvedValue(signedIn);
        const onSignedIn = vi.fn();
        render(<JiraSignIn status={undefined} onSignedIn={onSignedIn} />);
        const button = screen.getByRole("button", { name: "Sign in" });
        expect(button).toBeDisabled();

        fill(" acme.atlassian.net ", "me@acme.dev", "ATATT-secret");
        await act(async () => fireEvent.click(button));

        expect(signIn).toHaveBeenCalledWith("acme.atlassian.net", "me@acme.dev", "ATATT-secret");
        expect(onSignedIn).toHaveBeenCalledWith(signedIn);
    });

    it("signs in through the browser first when the build can, and waits for it to come back", async () => {
        let finish: (status: JiraStatus) => void = () => {};
        signInWithBrowser.mockReturnValue({ done: new Promise<JiraStatus>((resolve) => (finish = resolve)), cancel: vi.fn() });
        const onSignedIn = vi.fn();
        render(<JiraSignIn status={signedOutWithBrowser} onSignedIn={onSignedIn} />);
        expect(screen.queryByPlaceholderText("ATATT…")).not.toBeInTheDocument();

        fireEvent.click(screen.getByRole("button", { name: "Continue with Atlassian" }));
        expect(signInWithBrowser).toHaveBeenCalledOnce();
        expect(screen.getByText("Finish signing in in your browser")).toBeInTheDocument();

        await act(async () => finish(signedIn));
        expect(onSignedIn).toHaveBeenCalledWith(signedIn);
    });
});
