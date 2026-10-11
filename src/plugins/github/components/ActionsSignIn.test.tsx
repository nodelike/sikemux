import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ActionsStatus } from "../api";

const { signIn, openUrl } = vi.hoisted(() => ({ signIn: vi.fn(), openUrl: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), actionsApi: { signIn } }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), openUrl }));

import { ActionsSignIn } from "./ActionsSignIn";

const status: ActionsStatus = {
    configured: false,
    account: null,
    host: "github.com",
    login: "",
    tokenSource: null,
    tokenVariable: null,
    scopes: [],
    canWriteWorkflows: false,
    ok: false,
    authFailed: false,
    message: null,
};

afterEach(cleanup);

describe("ActionsSignIn", () => {
    it("signs in once however many times Enter is pressed while it checks", async () => {
        signIn.mockReset().mockReturnValue(new Promise(() => {}));
        render(<ActionsSignIn status={status} onSignedIn={() => {}} />);
        const token = screen.getByPlaceholderText("ghp_… or github_pat_…");
        fireEvent.change(token, { target: { value: "ghp_secret" } });
        await act(async () => {
            fireEvent.keyDown(token, { key: "Enter" });
            fireEvent.keyDown(token, { key: "Enter" });
        });
        expect(signIn).toHaveBeenCalledTimes(1);
        expect(signIn).toHaveBeenCalledWith("github.com", "ghp_secret");
    });

    it("names the variable an environment token came from", () => {
        render(
            <ActionsSignIn
                status={{ ...status, host: "ghe.corp", tokenSource: "environment", tokenVariable: "GH_ENTERPRISE_TOKEN" }}
                onSignedIn={() => {}}
            />,
        );
        expect(screen.getByText(/GH_ENTERPRISE_TOKEN is set in your shell/)).toBeTruthy();
    });

    it("does not sign in with an empty token", () => {
        signIn.mockReset();
        render(<ActionsSignIn status={status} onSignedIn={() => {}} />);
        fireEvent.keyDown(screen.getByPlaceholderText("ghp_… or github_pat_…"), { key: "Enter" });
        expect(signIn).not.toHaveBeenCalled();
    });
});

describe("signing in", () => {
    const tokenBox = () => screen.getByPlaceholderText("ghp_… or github_pat_…");

    it("hands over the account a pasted token signed in as", async () => {
        signIn.mockReset().mockResolvedValue({ ...status, ok: true, account: "ada-id" });
        const onSignedIn = vi.fn();
        render(<ActionsSignIn status={status} onSignedIn={onSignedIn} />);
        await userEvent.type(tokenBox(), "  ghp_secret  ");
        await userEvent.click(screen.getByRole("button", { name: "Sign in with token" }));
        expect(signIn).toHaveBeenCalledWith("github.com", "ghp_secret");
        expect(onSignedIn).toHaveBeenCalledWith("ada-id");
    });

    it("continues with the gh CLI's token without asking for one", async () => {
        signIn.mockReset().mockResolvedValue({ ...status, ok: true, account: null });
        const onSignedIn = vi.fn();
        render(<ActionsSignIn status={{ ...status, tokenSource: "ghCli" }} onSignedIn={onSignedIn} />);
        expect(screen.getByText("The gh CLI is signed in here, so Sikemux uses its token.")).toBeTruthy();
        await userEvent.click(screen.getByRole("button", { name: "Continue with the gh CLI" }));
        expect(signIn).toHaveBeenCalledWith("github.com", undefined);
        expect(onSignedIn).toHaveBeenCalledWith(null);
    });

    it("signs in to a company's own GitHub, and makes its tokens there", async () => {
        signIn.mockReset().mockResolvedValue({ ...status, ok: true, account: "corp-id" });
        openUrl.mockReset().mockResolvedValue(undefined);
        render(<ActionsSignIn status={status} onSignedIn={() => {}} />);
        expect(screen.queryByText("Host")).toBeNull();
        await userEvent.click(screen.getByRole("button", { name: "GitHub Enterprise" }));
        const hostBox = screen.getByPlaceholderText("github.com");
        await userEvent.clear(hostBox);
        await userEvent.type(hostBox, " ghe.corp ");
        await userEvent.click(screen.getByRole("button", { name: "Create a token" }));
        expect(openUrl).toHaveBeenCalledWith("https://ghe.corp/settings/tokens/new?scopes=repo,workflow&description=Sikemux");
        await userEvent.type(tokenBox(), "ghp_corp");
        await userEvent.click(screen.getByRole("button", { name: "Sign in with token" }));
        expect(signIn).toHaveBeenCalledWith("ghe.corp", "ghp_corp");
    });
});
