import { useState } from "react";
import { reportError } from "../state/toast";
import type { ProviderProfile } from "../state/types";
import { cancelSignIn, dismissSignIn, signIn, submitSignInCode, useSignIn } from "./accounts";
import { openUrl } from "../api/system";

/**
 * Signs one account in through its CLI's own login. While the browser is out,
 * it offers the page again and a place for the code some sign-ins show.
 */
export function AccountSignIn({
    profile,
    label = "Sign in",
    onSignedIn,
    buttonClass,
}: {
    profile: ProviderProfile;
    label?: string;
    onSignedIn?: () => void;
    /** Where the buttons should look like the controls around them, such as in Settings. */
    buttonClass?: string;
}) {
    const state = useSignIn(profile.id);
    const [code, setCode] = useState<string | null>(null);

    if (state?.phase === "waiting") {
        return (
            <span className="account-sign-in waiting">
                <span className="account-sign-in-note">Finish signing in in your browser</span>
                {state.url && (
                    <button type="button" className={buttonClass} onClick={() => void openUrl(state.url!).catch(reportError("Open sign-in page"))}>
                        Open page
                    </button>
                )}
                {state.url &&
                    (code === null ? (
                        <button type="button" className={buttonClass} onClick={() => setCode("")}>
                            Paste code
                        </button>
                    ) : (
                        <form
                            className="account-sign-in-code"
                            onSubmit={(event) => {
                                event.preventDefault();
                                if (!code.trim()) return;
                                void submitSignInCode(profile, code)
                                    .then(() => setCode(null))
                                    .catch(reportError("Sign-in code"));
                            }}>
                            <input
                                autoFocus
                                aria-label="Sign-in code"
                                placeholder="Code from the page"
                                value={code}
                                onChange={(event) => setCode(event.target.value)}
                            />
                        </form>
                    ))}
                <button type="button" className={buttonClass} onClick={() => cancelSignIn(profile)}>
                    Cancel
                </button>
            </span>
        );
    }

    const start = () => {
        dismissSignIn(profile.id);
        void signIn(profile).then((done) => {
            if (done) onSignedIn?.();
        });
    };
    return (
        <span className="account-sign-in">
            {state?.phase === "failed" && (
                <span className="account-sign-in-note failed" title={state.message}>
                    {state.message}
                </span>
            )}
            <button type="button" className={buttonClass ? `${buttonClass} primary` : undefined} onClick={start}>
                {state?.phase === "failed" ? "Try again" : label}
            </button>
        </span>
    );
}
