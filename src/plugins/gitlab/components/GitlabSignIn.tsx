import { useRef, useState, type KeyboardEvent } from "react";
import { openUrl, swallow } from "../../../plugin-api/host";
import { SignInScreen } from "../../../plugin-api/ui";
import { failureMessage, gitlabApi } from "../api";
import { GitlabMark } from "./GitlabMark";

/** Where to make a token: the server's own settings page, which is gitlab.com's unless another server is named. */
export function tokensPage(host: string): string {
    const server =
        host
            .trim()
            .replace(/^https:\/\//, "")
            .split("/")[0] || "gitlab.com";
    return `https://${server}/-/user_settings/personal_access_tokens?name=Sikemux&scopes=api,read_user`;
}

/** A personal access token on gitlab.com, or on a company's own server named above it. */
export function GitlabSignIn({ onSignedIn }: { onSignedIn: (account: string | null) => void }) {
    const [host, setHost] = useState("");
    const [token, setToken] = useState("");
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const checking = useRef(false);

    const canSubmit = !busy && !!token.trim();
    const submit = async () => {
        if (checking.current || !canSubmit) return;
        checking.current = true;
        setBusy(true);
        setError(null);
        try {
            const result = await gitlabApi.signInWithToken(token.trim(), host.trim());
            if (result.ok) onSignedIn(result.account);
            else setError(result.message ?? "GitLab turned that token down");
        } catch (failure) {
            setError(failureMessage(failure));
        } finally {
            checking.current = false;
            setBusy(false);
        }
    };
    const onEnter = (event: KeyboardEvent) => {
        if (event.key === "Enter") void submit();
    };

    return (
        <SignInScreen
            mark={<GitlabMark size={26} className="icon-gitlab" />}
            title="Connect GitLab"
            lede="Merge requests, pipelines, issues and your To-Do list for this project, right beside your changes."
            foot="Sikemux keeps your token in the macOS Keychain.">
            <div className="signin-form">
                <label className="signin-field">
                    <span>Server</span>
                    <input
                        className="signin-input"
                        value={host}
                        onChange={(event) => setHost(event.target.value)}
                        onKeyDown={onEnter}
                        placeholder="gitlab.com"
                        spellCheck={false}
                        autoCapitalize="off"
                        autoCorrect="off"
                    />
                    <small className="signin-hint">Leave empty for gitlab.com, or name your company's server, such as gitlab.acme.dev.</small>
                </label>
                <label className="signin-field">
                    <span>Personal access token</span>
                    <input
                        className="signin-input mono"
                        type="password"
                        value={token}
                        onChange={(event) => setToken(event.target.value)}
                        onKeyDown={onEnter}
                        placeholder="glpat-…"
                        autoFocus
                        spellCheck={false}
                    />
                    <small className="signin-hint">The api scope lets Sikemux start and retry pipelines; read_api only reads.</small>
                </label>
                <button type="button" className="signin-btn primary" disabled={!canSubmit} onClick={() => void submit()}>
                    {busy ? "Checking…" : "Sign in"}
                </button>
            </div>

            {error && (
                <div className="signin-callout" data-tone="danger">
                    {error}
                </div>
            )}

            <div className="signin-alt">
                <button type="button" className="signin-link" onClick={() => void openUrl(tokensPage(host)).catch(swallow("open GitLab"))}>
                    Create a token on {host.trim() || "gitlab.com"}
                </button>
            </div>
        </SignInScreen>
    );
}
