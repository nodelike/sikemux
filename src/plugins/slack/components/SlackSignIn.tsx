import { useState } from "react";
import { openUrl, swallow } from "../../../plugin-api/host";
import { SignInError, SignInScreen, useSignInAttempt } from "../../../plugin-api/ui";
import { slackApi, type SlackStatus } from "../api";
import { SlackMark } from "./SlackMark";

/** Where a person makes the Slack app whose token they paste here. */
export const SLACK_APPS_PAGE = "https://api.slack.com/apps";

/** A user token from a Slack app installed in the person's workspace; it reads and posts as them. */
export function SlackSignIn({ status, onSignedIn }: { status: SlackStatus | undefined; onSignedIn: (status: SlackStatus) => void }) {
    const [token, setToken] = useState("");
    const attempt = useSignInAttempt(status);
    const busy = attempt.busy;

    const canSubmit = !busy && !!token.trim();
    const submit = () => {
        if (!canSubmit) return;
        attempt.run(async () => onSignedIn(await slackApi.signIn(token.trim())));
    };

    return (
        <SignInScreen
            mark={<SlackMark size={26} />}
            title="Connect Slack"
            lede="Read a thread beside your code, hand it to an agent, and let the agent reply in it when the work is done."
            foot="Sikemux keeps the token in the macOS Keychain.">
            <div className="signin-form">
                <label className="signin-field">
                    <span>User OAuth Token</span>
                    <input
                        className="signin-input mono"
                        type="password"
                        value={token}
                        onChange={(event) => setToken(event.target.value)}
                        onKeyDown={(event) => {
                            if (event.key === "Enter") submit();
                        }}
                        placeholder="xoxp-…"
                        autoFocus
                        spellCheck={false}
                    />
                    <small className="signin-hint">
                        Make a Slack app, add the user scopes channels:history, groups:history, im:history, mpim:history, channels:read, groups:read,
                        im:read, mpim:read, chat:write, users:read, users:read.email and search:read, install it to your workspace, and copy the User
                        OAuth Token from OAuth &amp; Permissions.
                    </small>
                </label>
                <button type="button" className="signin-btn primary" disabled={!canSubmit} onClick={submit}>
                    {busy ? "Checking…" : "Sign in"}
                </button>
            </div>

            <SignInError error={attempt.error} />

            <div className="signin-alt">
                <button type="button" className="signin-link" onClick={() => void openUrl(SLACK_APPS_PAGE).catch(swallow("open Slack"))}>
                    Make a Slack app
                </button>
            </div>
        </SignInScreen>
    );
}
