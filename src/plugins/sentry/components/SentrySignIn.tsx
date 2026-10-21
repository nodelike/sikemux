import { useState } from "react";
import { openUrl, swallow } from "../../../plugin-api/host";
import { SignInError, SignInScreen, useSignInAttempt } from "../../../plugin-api/ui";
import { sentryApi, type SentryStatus } from "../api";
import { SentryMark } from "./SentryMark";

/** Where a person makes the personal token they paste here, on sentry.io or their own Sentry. */
export function tokensPage(url: string): string {
    const base = url.trim() ? (url.includes("://") ? url.trim() : `https://${url.trim()}`) : "https://sentry.io";
    return `${base.replace(/\/+$/, "")}/settings/account/api/auth-tokens/`;
}

export function SentrySignIn({ status, onSignedIn }: { status: SentryStatus | undefined; onSignedIn: (status: SentryStatus) => void }) {
    const [token, setToken] = useState("");
    const [url, setUrl] = useState(status?.url && status.url !== "https://sentry.io" ? status.url : "");
    const [selfHosted, setSelfHosted] = useState(url !== "");
    const attempt = useSignInAttempt(status);
    const busy = attempt.busy;
    const canSubmit = !busy && !!token.trim();
    const submit = () => {
        if (!canSubmit) return;
        attempt.run(async () => onSignedIn(await sentryApi.signIn(token.trim(), selfHosted ? url.trim() : "")));
    };

    return (
        <SignInScreen
            mark={<SentryMark size={26} className="icon-sentry" />}
            title="Connect Sentry"
            lede="See the errors your users hit, open the line that threw them, and hand an issue to an agent with its stack trace."
            foot="Sikemux keeps your token in the macOS Keychain.">
            <div className="signin-form">
                {selfHosted ? (
                    <label className="signin-field">
                        <span>Sentry address</span>
                        <input
                            className="signin-input mono"
                            value={url}
                            onChange={(event) => setUrl(event.target.value)}
                            placeholder="https://sentry.acme.dev"
                            spellCheck={false}
                        />
                    </label>
                ) : null}
                <label className="signin-field">
                    <span>Personal token</span>
                    <input
                        className="signin-input mono"
                        type="password"
                        value={token}
                        onChange={(event) => setToken(event.target.value)}
                        onKeyDown={(event) => {
                            if (event.key === "Enter") submit();
                        }}
                        placeholder="sntryu_…"
                        autoFocus
                        spellCheck={false}
                    />
                    <small className="signin-hint">
                        In Sentry, open User Settings → Personal Tokens, make one with the event:write, project:read, org:read and member:read scopes,
                        and paste it here.
                    </small>
                </label>
                <button type="button" className="signin-btn primary" disabled={!canSubmit} onClick={submit}>
                    {busy ? "Checking…" : "Sign in"}
                </button>
            </div>

            <SignInError error={attempt.error} />

            <div className="signin-alt">
                <button
                    type="button"
                    className="signin-link"
                    onClick={() => void openUrl(tokensPage(selfHosted ? url : "")).catch(swallow("open Sentry"))}>
                    Make a token
                </button>
                <button type="button" className="signin-link" onClick={() => setSelfHosted(!selfHosted)}>
                    {selfHosted ? "Use sentry.io" : "Self-hosted Sentry"}
                </button>
            </div>
        </SignInScreen>
    );
}
