import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const fake = vi.hoisted(() => ({ signIn: vi.fn(), openUrl: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), slackApi: { signIn: fake.signIn } }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), openUrl: fake.openUrl }));

import { SLACK_APPS_PAGE, SlackSignIn } from "./SlackSignIn";

afterEach(() => {
    cleanup();
    vi.clearAllMocks();
});

describe("SlackSignIn", () => {
    it("signs in with a user token", async () => {
        const status = { configured: true, ok: true, authFailed: false, message: null, workspaces: [] };
        fake.signIn.mockResolvedValue(status);
        const onSignedIn = vi.fn();
        render(<SlackSignIn status={undefined} onSignedIn={onSignedIn} />);
        fireEvent.change(screen.getByPlaceholderText("xoxp-…"), { target: { value: " xoxp-1-2 " } });
        await act(async () => fireEvent.click(screen.getByRole("button", { name: "Sign in" })));
        expect(fake.signIn).toHaveBeenCalledWith("xoxp-1-2");
        expect(onSignedIn).toHaveBeenCalledWith(status);
    });

    it("says why Slack refused, and opens the apps page", async () => {
        fake.signIn.mockRejectedValue({ category: "bad-params", message: "invalid argument: that is not a Slack token" });
        fake.openUrl.mockResolvedValue(undefined);
        render(<SlackSignIn status={undefined} onSignedIn={vi.fn()} />);
        fireEvent.change(screen.getByPlaceholderText("xoxp-…"), { target: { value: "glpat-x" } });
        await act(async () => fireEvent.keyDown(screen.getByPlaceholderText("xoxp-…"), { key: "Enter" }));
        expect(screen.getByText(/not a Slack token/)).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "Make a Slack app" }));
        expect(fake.openUrl).toHaveBeenCalledWith(SLACK_APPS_PAGE);
    });
});
