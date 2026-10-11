import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const fake = vi.hoisted(() => ({ signInWithToken: vi.fn(), openUrl: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), gitlabApi: { signInWithToken: fake.signInWithToken } }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), openUrl: fake.openUrl }));

import { GitlabSignIn, tokensPage } from "./GitlabSignIn";

afterEach(() => {
    cleanup();
    vi.clearAllMocks();
});

describe("GitlabSignIn", () => {
    it("signs in to a company server with a token", async () => {
        fake.signInWithToken.mockResolvedValue({ ok: true, account: "gitlab.acme.dev#3" });
        const onSignedIn = vi.fn();
        render(<GitlabSignIn onSignedIn={onSignedIn} />);
        expect(screen.getByRole("button", { name: "Sign in" })).toBeDisabled();
        fireEvent.change(screen.getByPlaceholderText("gitlab.com"), { target: { value: "gitlab.acme.dev" } });
        fireEvent.change(screen.getByPlaceholderText("glpat-…"), { target: { value: " glpat-abc " } });
        await act(async () => fireEvent.click(screen.getByRole("button", { name: "Sign in" })));
        expect(fake.signInWithToken).toHaveBeenCalledWith("glpat-abc", "gitlab.acme.dev");
        expect(onSignedIn).toHaveBeenCalledWith("gitlab.acme.dev#3");
    });

    it("opens the token page of the server named, or gitlab.com's", () => {
        fake.openUrl.mockResolvedValue(undefined);
        render(<GitlabSignIn onSignedIn={vi.fn()} />);
        fireEvent.click(screen.getByRole("button", { name: "Create a token on gitlab.com" }));
        expect(fake.openUrl).toHaveBeenCalledWith(tokensPage(""));
        expect(tokensPage("")).toBe("https://gitlab.com/-/user_settings/personal_access_tokens?name=Sikemux&scopes=api,read_user");
        expect(tokensPage("https://gitlab.acme.dev/users/sign_in")).toMatch(/^https:\/\/gitlab\.acme\.dev\/-\/user_settings/);
    });
});
