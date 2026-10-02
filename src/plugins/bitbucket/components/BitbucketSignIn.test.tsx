import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { BitbucketStatus } from "../api";

const fake = vi.hoisted(() => ({ signInWithToken: vi.fn(), signInWithBrowser: vi.fn(), openUrl: vi.fn(), status: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({
    ...(await importOriginal<object>()),
    bitbucketApi: { signInWithToken: fake.signInWithToken, signInWithBrowser: fake.signInWithBrowser, status: fake.status },
}));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), openUrl: fake.openUrl }));

import { BitbucketSignIn, SignInForm } from "./BitbucketSignIn";

const status: BitbucketStatus = {
    configured: false,
    account: null,
    method: null,
    login: "",
    displayName: null,
    avatarUrl: null,
    canWriteCi: false,
    ok: false,
    authFailed: false,
    message: null,
    browserSignIn: true,
};

const signedIn: BitbucketStatus = { ...status, account: "ada-id", configured: true, method: "oauth", login: "ada", ok: true, canWriteCi: true };

afterEach(cleanup);

describe("SignInForm", () => {
    it("opens Bitbucket's page in the browser and signs in once it comes back", async () => {
        fake.openUrl.mockReset().mockResolvedValue(undefined);
        let finish: (status: BitbucketStatus) => void = () => {};
        fake.signInWithBrowser.mockReset().mockImplementation((openPage: (url: string) => void) => {
            openPage("https://bitbucket.org/site/oauth2/authorize?client_id=x");
            return { done: new Promise((resolve) => (finish = resolve)), cancel: vi.fn() };
        });
        const onSignedIn = vi.fn();
        render(<SignInForm status={status} onSignedIn={onSignedIn} />);
        fireEvent.click(screen.getByRole("button", { name: "Continue with Bitbucket" }));
        expect(fake.openUrl).toHaveBeenCalledWith("https://bitbucket.org/site/oauth2/authorize?client_id=x");
        expect(screen.getByText(/Finish signing in in your browser/)).toBeTruthy();
        await act(async () => finish(signedIn));
        expect(onSignedIn).toHaveBeenCalledWith("ada-id");
    });

    it("stops waiting on the browser when told to", () => {
        const cancel = vi.fn();
        fake.signInWithBrowser.mockReset().mockReturnValue({ done: new Promise(() => {}), cancel });
        render(<SignInForm status={status} onSignedIn={() => {}} />);
        fireEvent.click(screen.getByRole("button", { name: "Continue with Bitbucket" }));
        fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
        expect(cancel).toHaveBeenCalled();
    });

    it("sends an API token with the email it belongs to, once however many times Enter is pressed", async () => {
        fake.signInWithToken.mockReset().mockReturnValue(new Promise(() => {}));
        render(<SignInForm status={{ ...status, browserSignIn: false }} onSignedIn={() => {}} />);
        const token = screen.getByPlaceholderText("ATATT… or ATCTT…");
        fireEvent.change(token, { target: { value: "ATATT3secret" } });
        fireEvent.change(screen.getByPlaceholderText("you@example.com"), { target: { value: " ada@example.com " } });
        await act(async () => {
            fireEvent.keyDown(token, { key: "Enter" });
            fireEvent.keyDown(token, { key: "Enter" });
        });
        expect(fake.signInWithToken).toHaveBeenCalledTimes(1);
        expect(fake.signInWithToken).toHaveBeenCalledWith("ATATT3secret", "ada@example.com");
    });

    it("sends an access token on its own when no email is given", async () => {
        fake.signInWithToken.mockReset().mockResolvedValue(signedIn);
        const onSignedIn = vi.fn();
        render(<SignInForm status={status} onSignedIn={onSignedIn} />);
        fireEvent.click(screen.getByRole("button", { name: "Use a token instead" }));
        fireEvent.change(screen.getByPlaceholderText("ATATT… or ATCTT…"), { target: { value: "ATCTTrepo" } });
        await act(async () => fireEvent.click(screen.getByRole("button", { name: "Sign in" })));
        expect(fake.signInWithToken).toHaveBeenCalledWith("ATCTTrepo", null);
        expect(onSignedIn).toHaveBeenCalledTimes(1);
    });
});

describe("when signing in goes wrong", () => {
    const browserAttempt = () => {
        let settle: { resolve: (status: BitbucketStatus) => void; reject: (error: unknown) => void } = { resolve: () => {}, reject: () => {} };
        const cancel = vi.fn();
        fake.signInWithBrowser.mockReset().mockReturnValue({
            done: new Promise<BitbucketStatus>((resolve, reject) => (settle = { resolve, reject })),
            cancel,
        });
        return { settle: () => settle, cancel };
    };

    it("says why Bitbucket refused the account the browser came back with, in its words or ours", async () => {
        const attempt = browserAttempt();
        const onSignedIn = vi.fn();
        render(<SignInForm status={status} onSignedIn={onSignedIn} />);
        fireEvent.click(screen.getByRole("button", { name: "Continue with Bitbucket" }));
        await act(async () => attempt.settle().resolve({ ...status, message: "Workspace blocks outside apps" }));
        expect(screen.getByText("Workspace blocks outside apps")).toBeTruthy();

        const again = browserAttempt();
        fireEvent.click(screen.getByRole("button", { name: "Continue with Bitbucket" }));
        expect(screen.queryByText("Workspace blocks outside apps")).toBeNull();
        await act(async () => again.settle().resolve(status));
        expect(screen.getByText("Bitbucket did not let that account in")).toBeTruthy();
        expect(onSignedIn).not.toHaveBeenCalled();
    });

    it("says why the browser sign-in failed, but not when it was called off", async () => {
        const failed = browserAttempt();
        render(<SignInForm status={status} onSignedIn={() => {}} />);
        fireEvent.click(screen.getByRole("button", { name: "Continue with Bitbucket" }));
        await act(async () => failed.settle().reject({ category: "timeout", message: "bitbucket: the browser never came back" }));
        expect(screen.getByText("bitbucket: the browser never came back")).toBeTruthy();

        const called = browserAttempt();
        fireEvent.click(screen.getByRole("button", { name: "Continue with Bitbucket" }));
        await act(async () => called.settle().reject(new Error("cancelled")));
        expect(document.querySelector(".signin-callout")).toBeNull();
        expect(screen.getByRole("button", { name: "Continue with Bitbucket" })).toBeTruthy();
    });

    it("stops waiting on the browser when the form goes away", () => {
        const attempt = browserAttempt();
        const { unmount } = render(<SignInForm status={status} onSignedIn={() => {}} />);
        fireEvent.click(screen.getByRole("button", { name: "Continue with Bitbucket" }));
        expect(screen.getByRole("button", { name: "Use a token instead" })).toHaveProperty("disabled", true);
        unmount();
        expect(attempt.cancel).toHaveBeenCalled();
    });

    it("says why a token was turned down, in Bitbucket's words or ours", async () => {
        fake.signInWithToken
            .mockReset()
            .mockResolvedValueOnce({ ...status, message: "Token expired" })
            .mockResolvedValueOnce(status);
        render(<SignInForm status={{ ...status, browserSignIn: false }} onSignedIn={() => {}} />);
        fireEvent.change(screen.getByPlaceholderText("ATATT… or ATCTT…"), { target: { value: "ATATTold" } });
        await act(async () => fireEvent.click(screen.getByRole("button", { name: "Sign in" })));
        expect(screen.getByText("Token expired")).toBeTruthy();
        await act(async () => fireEvent.keyDown(screen.getByPlaceholderText("you@example.com"), { key: "Enter" }));
        expect(screen.getByText("Bitbucket turned that token down")).toBeTruthy();
    });

    it("says why a token could not be checked", async () => {
        fake.signInWithToken.mockReset().mockRejectedValue({ category: "network", message: "bitbucket: offline" });
        render(<SignInForm status={{ ...status, browserSignIn: false }} onSignedIn={() => {}} />);
        fireEvent.change(screen.getByPlaceholderText("ATATT… or ATCTT…"), { target: { value: "ATATT" } });
        await act(async () => fireEvent.click(screen.getByRole("button", { name: "Sign in" })));
        expect(screen.getByText("bitbucket: offline")).toBeTruthy();
    });

    it("does not send an empty token", () => {
        fake.signInWithToken.mockReset();
        render(<SignInForm status={{ ...status, browserSignIn: false }} onSignedIn={() => {}} />);
        fireEvent.change(screen.getByPlaceholderText("ATATT… or ATCTT…"), { target: { value: "   " } });
        fireEvent.keyDown(screen.getByPlaceholderText("ATATT… or ATCTT…"), { key: "Enter" });
        expect(fake.signInWithToken).not.toHaveBeenCalled();
    });

    it("shows why a stored sign-in stopped working", () => {
        render(<SignInForm status={{ ...status, authFailed: true, message: "Refresh token revoked" }} onSignedIn={() => {}} />);
        expect(screen.getByText("Refresh token revoked")).toBeTruthy();
    });
});

describe("the other way in", () => {
    it("goes back to the browser from the token form", () => {
        render(<SignInForm status={status} onSignedIn={() => {}} />);
        fireEvent.click(screen.getByRole("button", { name: "Use a token instead" }));
        fireEvent.click(screen.getByRole("button", { name: "Sign in with the browser instead" }));
        expect(screen.getByRole("button", { name: "Continue with Bitbucket" })).toBeTruthy();
    });

    it("links to making an API token when the browser is not an option", () => {
        fake.openUrl.mockReset().mockResolvedValue(undefined);
        render(<SignInForm status={{ ...status, browserSignIn: false }} onSignedIn={() => {}} />);
        fireEvent.click(screen.getByRole("button", { name: "Create an API token" }));
        expect(fake.openUrl).toHaveBeenCalledWith("https://id.atlassian.com/manage-profile/security/api-tokens");
    });
});

describe("BitbucketSignIn", () => {
    it("connects before showing the form", async () => {
        fake.status.mockReset().mockResolvedValue({ ...status, browserSignIn: false });
        render(<BitbucketSignIn onSignedIn={() => {}} />);
        expect(screen.getByRole("status", { name: "Connecting to Bitbucket" })).toBeTruthy();
        expect(await screen.findByRole("button", { name: "Create an API token" })).toBeTruthy();
    });
});
