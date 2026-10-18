import { useState } from "react";
import { openSignInUrl, openUrl, swallow } from "../../../plugin-api/host";
import { resource, useResource } from "../../../plugin-api/resources";
import { SignInError, SignInScreen, SignInWaiting, SkeletonRows, useSignInAttempt } from "../../../plugin-api/ui";
import { bitbucketApi, type BitbucketStatus } from "../api";
import { BitbucketMark } from "./BitbucketMark";

const API_TOKENS_PAGE = "https://id.atlassian.com/manage-profile/security/api-tokens";

/** The sign-in form wants to know whether this build can sign in through the browser, which only Bitbucket's own status says. */
const bitbucketStatusR = resource({
    kind: "host.bitbucketStatus",
    fetch: (): Promise<BitbucketStatus> => bitbucketApi.status(),
    staleAfterMs: 60_000,
});

export function BitbucketSignIn({ onSignedIn }: { onSignedIn: (account: string | null) => void }) {
    const status = useResource(bitbucketStatusR);
    if (!status.data) return <SkeletonRows rows={4} label="Connecting to Bitbucket" />;
    return <SignInForm status={status.data} onSignedIn={onSignedIn} />;
}

interface Props {
    status: BitbucketStatus;
    onSignedIn: (account: string | null) => void;
}

/** Signing in through the browser comes first; a pasted token is for workspaces that turn outside apps away. */
export function SignInForm({ status, onSignedIn }: Props) {
    const [withToken, setWithToken] = useState(!status.browserSignIn);
    const [token, setToken] = useState("");
    const [email, setEmail] = useState("");
    const attempt = useSignInAttempt(status);
    const busy = attempt.busy;

    const signInWithBrowser = () =>
        attempt.follow(
            bitbucketApi.signInWithBrowser((url) => void openSignInUrl(url).catch(swallow("open Bitbucket"))),
            (result) => {
                if (!result.ok) return result.message ?? "Bitbucket did not let that account in";
                onSignedIn(result.account);
            },
        );

    const canSubmit = !busy && !!token.trim();
    const submit = () => {
        if (!canSubmit) return;
        attempt.run(async () => {
            const result = await bitbucketApi.signInWithToken(token.trim(), email.trim() || null);
            if (!result.ok) return result.message ?? "Bitbucket turned that token down";
            onSignedIn(result.account);
        });
    };

    return (
        <SignInScreen
            mark={<BitbucketMark size={26} />}
            title="Connect Bitbucket"
            lede="Pull requests, pipelines and reviews for this repository, right beside your changes."
            foot="Sikemux keeps your sign-in in the macOS Keychain.">
            {withToken ? (
                <div className="signin-form">
                    <label className="signin-field">
                        <span>Token</span>
                        <input
                            className="signin-input mono"
                            type="password"
                            value={token}
                            onChange={(event) => setToken(event.target.value)}
                            onKeyDown={(event) => {
                                if (event.key === "Enter") submit();
                            }}
                            placeholder="ATATT… or ATCTT…"
                            autoFocus
                            spellCheck={false}
                        />
                    </label>
                    <label className="signin-field">
                        <span>Atlassian account email</span>
                        <input
                            className="signin-input"
                            type="email"
                            value={email}
                            onChange={(event) => setEmail(event.target.value)}
                            onKeyDown={(event) => {
                                if (event.key === "Enter") submit();
                            }}
                            placeholder="you@example.com"
                            spellCheck={false}
                            autoCapitalize="off"
                            autoCorrect="off"
                        />
                        <small className="signin-hint">
                            An API token needs the email it belongs to. Leave it empty for a repository or workspace access token.
                        </small>
                    </label>
                    <button type="button" className="signin-btn primary" disabled={!canSubmit} onClick={submit}>
                        {busy ? "Checking…" : "Sign in"}
                    </button>
                </div>
            ) : attempt.waiting ? (
                <SignInWaiting onCancel={attempt.cancel}>Finish signing in in your browser</SignInWaiting>
            ) : (
                <button type="button" className="signin-btn primary" onClick={signInWithBrowser}>
                    <BitbucketMark size={14} mono />
                    Continue with Bitbucket
                </button>
            )}

            <SignInError error={attempt.error} />

            <div className="signin-alt">
                {withToken ? (
                    status.browserSignIn ? (
                        <button type="button" className="signin-link" onClick={() => setWithToken(false)}>
                            Sign in with the browser instead
                        </button>
                    ) : (
                        <button type="button" className="signin-link" onClick={() => void openUrl(API_TOKENS_PAGE).catch(swallow("open Atlassian"))}>
                            Create an API token
                        </button>
                    )
                ) : (
                    <button type="button" className="signin-link" disabled={attempt.waiting} onClick={() => setWithToken(true)}>
                        Use a token instead
                    </button>
                )}
            </div>
        </SignInScreen>
    );
}
