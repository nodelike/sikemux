import { useState } from "react";
import { openSignInUrl, openUrl, swallow } from "../../../plugin-api/host";
import { SignInError, SignInScreen, SignInWaiting, useSignInAttempt } from "../../../plugin-api/ui";
import { linearApi, type LinearStatus } from "../api";
import { LinearMark } from "./LinearMark";

/** Where a person makes the personal API key they paste here. */
export const LINEAR_KEYS_PAGE = "https://linear.app/settings/account/security";

/** Signing in through the browser comes first; a personal API key is for workspaces that turn outside apps away. */
export function LinearSignIn({ status, onSignedIn }: { status: LinearStatus | undefined; onSignedIn: (status: LinearStatus) => void }) {
    const [withKey, setWithKey] = useState(false);
    const [key, setKey] = useState("");
    const attempt = useSignInAttempt(status);
    const busy = attempt.busy;

    const signInWithBrowser = () =>
        attempt.follow(
            linearApi.signInWithBrowser((url) => void openSignInUrl(url).catch(swallow("open Linear"))),
            (result) => onSignedIn(result),
        );

    const canSubmit = !busy && !!key.trim();
    const submit = () => {
        if (!canSubmit) return;
        attempt.run(async () => onSignedIn(await linearApi.signIn(key.trim())));
    };

    return (
        <SignInScreen
            mark={<LinearMark size={26} className="icon-linear" />}
            title="Connect Linear"
            lede="See the issues you are meant to be working on beside your code, and let agents read, comment on and move them."
            foot="Sikemux keeps your sign-in in the macOS Keychain.">
            {withKey ? (
                <div className="signin-form">
                    <label className="signin-field">
                        <span>Personal API key</span>
                        <input
                            className="signin-input mono"
                            type="password"
                            value={key}
                            onChange={(event) => setKey(event.target.value)}
                            onKeyDown={(event) => {
                                if (event.key === "Enter") submit();
                            }}
                            placeholder="lin_api_…"
                            autoFocus
                            spellCheck={false}
                        />
                        <small className="signin-hint">
                            In Linear, open Settings → Security &amp; access, make a personal API key, and paste it here.
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
                    <LinearMark size={14} />
                    Continue with Linear
                </button>
            )}

            <SignInError error={attempt.error} />

            <div className="signin-alt">
                {withKey ? (
                    <>
                        <button type="button" className="signin-link" onClick={() => setWithKey(false)}>
                            Sign in with the browser instead
                        </button>
                        <button type="button" className="signin-link" onClick={() => void openUrl(LINEAR_KEYS_PAGE).catch(swallow("open Linear"))}>
                            Make an API key
                        </button>
                    </>
                ) : (
                    <button type="button" className="signin-link" disabled={attempt.waiting} onClick={() => setWithKey(true)}>
                        Use an API key instead
                    </button>
                )}
            </div>
        </SignInScreen>
    );
}
