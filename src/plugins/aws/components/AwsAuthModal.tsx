import { useEffect, useRef, useState } from "react";
import { focusSignInBrowser, openSignInUrl, swallow } from "../../../plugin-api/host";
import { invalidate } from "../../../plugin-api/resources";
import { IconClose } from "../../../plugin-api/ui";
import { awsApi, type SsoLogin } from "../api";
import { awsIdentityR } from "../resources";
import { closeAwsAuthModal, useAws } from "../state";
import "../auth.css";

async function signedIn(profile: string): Promise<void> {
    await awsApi.identity(profile, true).catch(swallow("refresh AWS identity"));
    invalidate((kind, args) => kind === awsIdentityR.kind && args[0] === profile);
}

export function AwsAuthModal() {
    const modal = useAws((s) => s.authModal);

    const [phase, setPhase] = useState<"idle" | "running" | "ok" | "fail">("idle");
    const [errOut, setErrOut] = useState("");
    const loginRef = useRef<SsoLogin | null>(null);

    useEffect(() => {
        if (!modal) return;
        setPhase("idle");
        setErrOut("");
        return () => {
            loginRef.current?.cancel();
            loginRef.current = null;
        };
    }, [modal]);

    if (!modal) return null;

    const openInBrowser = (url: string) => void openSignInUrl(url).catch(swallow("open the SSO portal"));

    const onCancel = () => {
        loginRef.current?.cancel();
        loginRef.current = null;
        closeAwsAuthModal();
    };

    const onSignIn = async () => {
        setPhase("running");
        setErrOut("");
        await focusSignInBrowser().catch(swallow("bring the sign-in browser forward"));
        const login = awsApi.ssoLogin(modal.profile);
        loginRef.current = login;
        try {
            const result = await login.result;
            if (loginRef.current !== login) return;
            if (result.success) await signedIn(modal.profile);
            setPhase(result.success ? "ok" : "fail");
            if (!result.success) setErrOut(result.stderr.trim());
            if (result.success) window.setTimeout(closeAwsAuthModal, 700);
        } catch (e) {
            if (loginRef.current !== login) return;
            setPhase("fail");
            setErrOut(String(e));
        } finally {
            if (loginRef.current === login) loginRef.current = null;
        }
    };

    return (
        <div className="settings-backdrop" onMouseDown={onCancel}>
            <div className="aws-auth-modal" onMouseDown={(e) => e.stopPropagation()}>
                <div className="settings-head">
                    <span className="settings-title">
                        <strong>·</strong>aws sign-in
                    </span>
                    <button className="settings-close" onClick={onCancel} title="Close">
                        <IconClose size={11} />
                    </button>
                </div>

                <div className="aws-auth-body">
                    <div className="aws-auth-row">
                        <span className="aws-auth-label">Profile</span>
                        <span className="aws-auth-value">{modal.profile}</span>
                    </div>
                    {modal.ssoStartUrl && (
                        <div className="aws-auth-row">
                            <span className="aws-auth-label">SSO portal</span>
                            <button
                                type="button"
                                className="aws-auth-link"
                                onClick={() => modal.ssoStartUrl && openInBrowser(modal.ssoStartUrl)}
                                title="Open in configured browser">
                                {modal.ssoStartUrl}
                            </button>
                        </div>
                    )}

                    <div className="aws-auth-steps">
                        <ol>
                            <li>
                                Click <strong>Sign in with SSO</strong> below — your browser opens automatically with the device-authorization code.
                            </li>
                            <li>Approve the request in your Identity Center tab.</li>
                            <li>
                                The dialog flips to ✓ as soon as <code>sts</code> succeeds.
                            </li>
                        </ol>
                    </div>

                    {phase === "running" && <div className="aws-auth-status running">waiting for browser approval…</div>}
                    {phase === "ok" && <div className="aws-auth-status ok">authenticated ✓</div>}
                    {phase === "fail" && (
                        <div className="aws-auth-status fail">
                            login failed — try again or run <code>aws sso login --profile {modal.profile}</code> in a terminal
                            {errOut && <pre className="aws-auth-err">{errOut}</pre>}
                        </div>
                    )}
                </div>

                <div className="aws-auth-actions">
                    <button className="settings-btn" onClick={onCancel}>
                        Cancel
                    </button>
                    <button
                        className="settings-btn primary"
                        onClick={onSignIn}
                        disabled={phase === "running" || phase === "ok"}
                        aria-busy={phase === "running"}>
                        {phase === "running" ? "Signing in…" : "Sign in with SSO"}
                    </button>
                </div>
            </div>
        </div>
    );
}
