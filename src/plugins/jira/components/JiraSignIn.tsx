import { useState } from "react";
import { openSignInUrl, openUrl, swallow } from "../../../plugin-api/host";
import { SignInError, SignInScreen, SignInWaiting, useSignInAttempt } from "../../../plugin-api/ui";
import { jiraApi, type JiraStatus } from "../api";
import { JiraMark } from "./JiraMark";

const API_TOKENS_PAGE = "https://id.atlassian.com/manage-profile/security/api-tokens";

/** Signing in through the browser comes first; an API token is for builds without it and sites that turn outside apps away. */
export function JiraSignIn({ status, onSignedIn }: { status: JiraStatus | undefined; onSignedIn: (status: JiraStatus) => void }) {
    const [choseToken, setChoseToken] = useState(false);
    const withToken = choseToken || !status?.browserSignIn;
    const [site, setSite] = useState("");
    const [email, setEmail] = useState("");
    const [token, setToken] = useState("");
    const attempt = useSignInAttempt(status);
    const busy = attempt.busy;

    const signInWithBrowser = () =>
        attempt.follow(
            jiraApi.signInWithBrowser((url) => void openSignInUrl(url).catch(swallow("open Atlassian"))),
            (result) => onSignedIn(result),
        );

    const canSubmit = !busy && !!site.trim() && !!email.trim() && !!token.trim();
    const submit = () => {
        if (!canSubmit) return;
        attempt.run(async () => onSignedIn(await jiraApi.signIn(site.trim(), email.trim(), token.trim())));
    };
    const onEnter = (event: { key: string }) => {
        if (event.key === "Enter") submit();
    };

    return (
        <SignInScreen
            mark={<JiraMark size={26} />}
            title="Connect Jira"
            lede="The tickets you are working on, beside the code, and tools for your agents to read and update them."
            foot="Sikemux keeps your sign-in in the macOS Keychain.">
            {withToken ? (
                <div className="signin-form">
                    <label className="signin-field">
                        <span>Jira site</span>
                        <input
                            className="signin-input"
                            value={site}
                            onChange={(event) => setSite(event.target.value)}
                            onKeyDown={onEnter}
                            placeholder="your-team.atlassian.net"
                            autoFocus
                            spellCheck={false}
                            autoCapitalize="off"
                            autoCorrect="off"
                        />
                    </label>
                    <label className="signin-field">
                        <span>Atlassian account email</span>
                        <input
                            className="signin-input"
                            type="email"
                            value={email}
                            onChange={(event) => setEmail(event.target.value)}
                            onKeyDown={onEnter}
                            placeholder="you@example.com"
                            spellCheck={false}
                            autoCapitalize="off"
                            autoCorrect="off"
                        />
                    </label>
                    <label className="signin-field">
                        <span>API token</span>
                        <input
                            className="signin-input mono"
                            type="password"
                            value={token}
                            onChange={(event) => setToken(event.target.value)}
                            onKeyDown={onEnter}
                            placeholder="ATATT…"
                            spellCheck={false}
                        />
                    </label>
                    <button type="button" className="signin-btn primary" disabled={!canSubmit} onClick={submit}>
                        {busy ? "Checking…" : "Sign in"}
                    </button>
                </div>
            ) : attempt.waiting ? (
                <SignInWaiting onCancel={attempt.cancel}>Finish signing in in your browser</SignInWaiting>
            ) : (
                <button type="button" className="signin-btn primary" onClick={signInWithBrowser}>
                    <JiraMark size={14} mono />
                    Continue with Atlassian
                </button>
            )}
            <SignInError error={attempt.error} />
            <div className="signin-alt">
                {withToken ? (
                    status?.browserSignIn ? (
                        <button type="button" className="signin-link" onClick={() => setChoseToken(false)}>
                            Sign in with the browser instead
                        </button>
                    ) : (
                        <button type="button" className="signin-link" onClick={() => void openUrl(API_TOKENS_PAGE).catch(swallow("open Atlassian"))}>
                            Create an API token
                        </button>
                    )
                ) : (
                    <button type="button" className="signin-link" disabled={attempt.waiting} onClick={() => setChoseToken(true)}>
                        Use an API token instead
                    </button>
                )}
            </div>
        </SignInScreen>
    );
}
