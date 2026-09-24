import { useState } from "react";
import { errorMessage, rundeckApi } from "../api";
import { Checkbox } from "../../../plugin-api/ui";

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
        <div className="rnd-login">
            <form
                className="rnd-login-card"
                onSubmit={(e) => {
                    e.preventDefault();
                    void submit();
                }}>
                <div className="rnd-login-title">
                    <span>connect to rundeck</span>
                </div>
                <div className="rnd-login-modes" role="radiogroup" aria-label="Sign-in method">
                    <ModeChip mode="password" current={mode} onPick={setMode}>
                        password
                    </ModeChip>
                    <ModeChip mode="token" current={mode} onPick={setMode}>
                        API token
                    </ModeChip>
                </div>
                <div className="rnd-login-help">
                    {mode === "password" ? (
                        <>
                            Signs in once to mint an API token, stored at <code>~/.rd-config</code> (chmod&nbsp;600) and shared with the{" "}
                            <code>rnd</code> CLI. Your password is never saved.
                        </>
                    ) : (
                        <>
                            Paste a token from your Rundeck profile page. It is stored at <code>~/.rd-config</code> (chmod&nbsp;600) and shared with
                            the <code>rnd</code> CLI.
                        </>
                    )}
                </div>

                <label className="rnd-field">
                    <span>Rundeck URL</span>
                    <input
                        type="url"
                        placeholder="http://rundeck.internal:4440"
                        value={url}
                        onChange={(e) => setUrl(e.target.value)}
                        {...textProps}
                    />
                </label>

                {insecureHttp && (
                    <div className="rnd-insecure-http">
                        <Checkbox checked={allowInsecurePrivateHttp} onChange={setAllowInsecurePrivateHttp}>
                            Allow plaintext HTTP for this private-subnet host. I understand the{" "}
                            {mode === "password" ? "password and token are" : "token is"} not protected by TLS. Sikemux will refuse the connection
                            unless every resolved address is private or loopback.
                        </Checkbox>
                    </div>
                )}

                {mode === "password" ? (
                    <>
                        <label className="rnd-field">
                            <span>Username</span>
                            <input type="text" autoComplete="username" value={user} onChange={(e) => setUser(e.target.value)} {...textProps} />
                        </label>
                        <label className="rnd-field">
                            <span>Password</span>
                            <input type="password" autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} />
                        </label>
                    </>
                ) : (
                    <label className="rnd-field">
                        <span>API token</span>
                        <input type="password" autoComplete="off" value={token} onChange={(e) => setToken(e.target.value)} {...textProps} />
                    </label>
                )}

                {error && <div className="rnd-login-error">{error}</div>}

                <button type="submit" className="rnd-btn rnd-btn-primary" disabled={!canSubmit}>
                    {busy ? "signing in…" : "sign in"}
                </button>
            </form>
        </div>
    );
}

function ModeChip({ mode, current, onPick, children }: { mode: Mode; current: Mode; onPick: (mode: Mode) => void; children: string }) {
    const on = mode === current;
    return (
        <button type="button" role="radio" aria-checked={on} className={`rnd-mode-chip${on ? " on" : ""}`} onClick={() => onPick(mode)}>
            {children}
        </button>
    );
}
