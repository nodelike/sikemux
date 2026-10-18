import { useState } from "react";
import { openSignInUrl, openUrl, swallow } from "../../../plugin-api/host";
import { SignInError, SignInScreen, SignInWaiting, useSignInAttempt } from "../../../plugin-api/ui";
import { vercelApi, type VercelStatus } from "../api";
import { VercelMark } from "./VercelMark";

/** Where a person makes the access token they paste here. */
export const VERCEL_TOKENS_PAGE = "https://vercel.com/account/settings/tokens";

/** Signing in through the browser installs the Sikemux integration on a team; an access token is for builds without it. */
export function VercelSignIn({ status, onSignedIn }: { status: VercelStatus | undefined; onSignedIn: (status: VercelStatus) => void }) {
    const browser = status?.browserSignIn ?? false;
    const [withToken, setWithToken] = useState(!browser);
    const [token, setToken] = useState("");
    const attempt = useSignInAttempt(status);

    const signInWithBrowser = () =>
        attempt.follow(
            vercelApi.signInWithBrowser((url) => void openSignInUrl(url).catch(swallow("open Vercel"))),
            (result) => onSignedIn(result),
        );

    const canSubmit = !attempt.busy && !!token.trim();
    const submit = () => {
        if (!canSubmit) return;
        attempt.run(async () => onSignedIn(await vercelApi.signIn(token.trim())));
    };

    return (
        <SignInScreen
            mark={<VercelMark size={26} className="icon-vercel" />}
            title="Connect Vercel"
            lede="See your projects, deployments and build logs beside your code, and let agents read them, redeploy and promote."
            foot="Sikemux keeps your sign-in in the macOS Keychain.">
            {withToken ? (
                <div className="signin-form">
                    <label className="signin-field">
                        <span>Access token</span>
                        <input
                            className="signin-input mono"
                            type="password"
                            value={token}
                            onChange={(event) => setToken(event.target.value)}
                            onKeyDown={(event) => {
                                if (event.key === "Enter") submit();
                            }}
                            placeholder="Paste a Vercel token"
                            autoFocus
                            spellCheck={false}
                        />
                        <small className="signin-hint">
                            In Vercel, open Account Settings → Tokens, create a token for the scopes you want, and paste it here.
                        </small>
                    </label>
                    <button type="button" className="signin-btn primary" disabled={!canSubmit} onClick={submit}>
                        {attempt.busy ? "Checking…" : "Sign in"}
                    </button>
                </div>
            ) : attempt.waiting ? (
                <SignInWaiting onCancel={attempt.cancel}>Finish signing in in your browser</SignInWaiting>
            ) : (
                <button type="button" className="signin-btn primary" onClick={signInWithBrowser}>
                    <VercelMark size={14} />
                    Continue with Vercel
                </button>
            )}

            <SignInError error={attempt.error} />

            <div className="signin-alt">
                {withToken ? (
                    <>
                        {browser && (
                            <button type="button" className="signin-link" onClick={() => setWithToken(false)}>
                                Sign in with the browser instead
                            </button>
                        )}
                        <button type="button" className="signin-link" onClick={() => void openUrl(VERCEL_TOKENS_PAGE).catch(swallow("open Vercel"))}>
                            Make a token
                        </button>
                    </>
                ) : (
                    <button type="button" className="signin-link" disabled={attempt.waiting} onClick={() => setWithToken(true)}>
                        Use an access token instead
                    </button>
                )}
            </div>
        </SignInScreen>
    );
}
