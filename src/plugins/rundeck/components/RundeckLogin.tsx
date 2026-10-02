import { useState } from "react";
import { errorMessage, rundeckApi } from "../api";
import { Checkbox, IconRundeck, SignInScreen } from "../../../plugin-api/ui";

interface Props {
    initialUrl?: string;
    initialUser?: string;
    initialAllowInsecurePrivateHttp?: boolean;
    notice?: string;
    onDone: () => void;
}

type Mode = "password" | "token";

const textProps = { spellCheck: false, autoCapitalize: "off", autoCorrect: "off" } as const;

export function RundeckLogin({ initialUrl = "", initialUser = "", initialAllowInsecurePrivateHttp = false, notice, onDone }: Props) {
    const [mode, setMode] = useState<Mode>("password");
    const [url, setUrl] = useState(initialUrl);
    const [user, setUser] = useState(initialUser);
    const [password, setPassword] = useState("");
    const [token, setToken] = useState("");
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(notice ?? null);
    const [allowInsecurePrivateHttp, setAllowInsecurePrivateHttp] = useState(initialAllowInsecurePrivateHttp);

    const trimmedUrl = url.trim();
    const insecureHttp = trimmedUrl.toLowerCase().startsWith("http://");
    const secretReady = mode === "password" ? !!user.trim() && password.length > 0 : !!token.trim();
    const canSubmit = !!trimmedUrl && secretReady && (!insecureHttp || allowInsecurePrivateHttp) && !busy;

    const submit = async () => {
        if (!canSubmit) return;
        setBusy(true);
        setError(null);
        const allow_insecure_private_http = insecureHttp && allowInsecurePrivateHttp;
        try {
            if (mode === "password") {
                await rundeckApi.login({ url: trimmedUrl, user: user.trim(), password, allow_insecure_private_http });
                setPassword("");
            } else {
                await rundeckApi.loginWithToken({ url: trimmedUrl, token: token.trim(), allow_insecure_private_http });
                setToken("");
            }
            onDone();
        } catch (e) {
            setError(errorMessage(e));
        } finally {
            setBusy(false);
        }
    };

    return (
        <div className="rnd-main">
            <SignInScreen
                mark={<IconRundeck size={26} />}
                title="Connect Rundeck"
                lede="Run jobs and follow their logs for this project, right beside your changes."
                foot={
                    <>
                        {mode === "password" ? "Your password is used once to mint an API token. " : ""}The token lives in <code>~/.rd-config</code>{" "}
                        and is shared with the <code>rnd</code> CLI.
                    </>
                }>
                <div className="signin-modes" role="radiogroup" aria-label="Sign-in method">
                    <ModeChip mode="password" current={mode} onPick={setMode}>
                        Password
                    </ModeChip>
                    <ModeChip mode="token" current={mode} onPick={setMode}>
                        API token
                    </ModeChip>
                </div>

                <form
                    className="signin-form"
                    onSubmit={(e) => {
                        e.preventDefault();
                        void submit();
                    }}>
                    <label className="signin-field">
                        <span>Rundeck URL</span>
                        <input
                            className="signin-input mono"
                            type="url"
                            placeholder="http://rundeck.internal:4440"
                            value={url}
                            onChange={(e) => setUrl(e.target.value)}
                            {...textProps}
                        />
                    </label>

                    {insecureHttp && (
                        <div className="signin-callout" data-tone="warn">
                            <Checkbox checked={allowInsecurePrivateHttp} onChange={setAllowInsecurePrivateHttp}>
                                Allow plaintext HTTP for this private-subnet host. I understand the{" "}
                                {mode === "password" ? "password and token are" : "token is"} not protected by TLS. Sikemux will refuse the connection
                                unless every resolved address is private or loopback.
                            </Checkbox>
                        </div>
                    )}

                    {mode === "password" ? (
                        <>
                            <label className="signin-field">
                                <span>Username</span>
                                <input
                                    className="signin-input"
                                    type="text"
                                    autoComplete="username"
                                    value={user}
                                    onChange={(e) => setUser(e.target.value)}
                                    {...textProps}
                                />
                            </label>
                            <label className="signin-field">
                                <span>Password</span>
                                <input
                                    className="signin-input"
                                    type="password"
                                    autoComplete="current-password"
                                    value={password}
                                    onChange={(e) => setPassword(e.target.value)}
                                />
                            </label>
                        </>
                    ) : (
                        <label className="signin-field">
                            <span>API token</span>
                            <input
                                className="signin-input mono"
                                type="password"
                                autoComplete="off"
                                value={token}
                                onChange={(e) => setToken(e.target.value)}
                                {...textProps}
                            />
                            <small className="signin-hint">Create one on your Rundeck profile page.</small>
                        </label>
                    )}

                    <button type="submit" className="signin-btn primary" disabled={!canSubmit}>
                        {busy ? "Signing in…" : "Sign in"}
                    </button>
                </form>

                {error && (
                    <div className="signin-callout" data-tone="danger">
                        {error}
                    </div>
                )}
            </SignInScreen>
        </div>
    );
}

function ModeChip({ mode, current, onPick, children }: { mode: Mode; current: Mode; onPick: (mode: Mode) => void; children: string }) {
    const on = mode === current;
    return (
        <button type="button" role="radio" aria-checked={on} className="signin-mode" onClick={() => onPick(mode)}>
            {children}
        </button>
    );
}
