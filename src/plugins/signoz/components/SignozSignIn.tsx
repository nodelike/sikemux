import { useEffect, useState } from "react";
import { openUrl, swallow } from "../../../plugin-api/host";
import { Checkbox, SignInScreen } from "../../../plugin-api/ui";
import { failureMessage, signozApi, type Inspection, type SignozStatus } from "../api";
import { SignozIcon } from "./SignozIcon";

type Method = "password" | "apiKey";

function hostOf(url: string): string {
    try {
        return new URL(url).host;
    } catch {
        return url;
    }
}

interface Props {
    status: SignozStatus;
    onSignedIn: () => void;
}

/**
 * The address first, because it answers everything else: which version this
 * is, and, once an email is typed, whether that person signs in with a
 * password there at all.
 */
export function SignozSignIn({ status, onSignedIn }: Props) {
    const [url, setUrl] = useState(status.url);
    const [email, setEmail] = useState(status.email);
    const [password, setPassword] = useState("");
    const [apiKey, setApiKey] = useState("");
    const [useKeychain, setUseKeychain] = useState(false);
    const [account, setAccount] = useState("");
    const [method, setMethod] = useState<Method>(status.auth === "apiKey" ? "apiKey" : "password");
    const [inspection, setInspection] = useState<Inspection | null>(null);
    const [orgId, setOrgId] = useState("");
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(status.authFailed ? status.message : null);
    const [editingUrl, setEditingUrl] = useState(!status.url);

    const look = (withEmail: boolean, settle = false) => {
        if (!url.trim()) return;
        setError(null);
        signozApi
            .inspect(url.trim(), withEmail ? email.trim() || undefined : undefined)
            .then((found) => {
                setInspection(found);
                setUrl(found.url);
                if (settle) setEditingUrl(false);
                const passwordOrgs = found.orgs.filter((org) => org.password);
                if (passwordOrgs.length === 1) setOrgId(passwordOrgs[0].id);
            })
            .catch((failure: unknown) => {
                setInspection(null);
                setEditingUrl(true);
                setError(failureMessage(failure));
            });
    };

    // A remembered address is checked straight away, so the version and how
    // this email signs in are already on screen.
    useEffect(() => {
        if (status.url) look(true);
        // eslint-disable-next-line react-hooks/exhaustive-deps -- only the address remembered when the form opens is checked
    }, []);

    const passwordOrgs = inspection?.orgs.filter((org) => org.password) ?? [];
    const ssoOnly = inspection !== null && inspection.orgs.length > 0 && passwordOrgs.length === 0;
    const canSubmit =
        !busy &&
        !!url.trim() &&
        (method === "password" ? !!email.trim() && password.length > 0 && !ssoOnly : useKeychain ? !!account.trim() : !!apiKey.trim());

    const submit = async () => {
        if (!canSubmit) return;
        setBusy(true);
        setError(null);
        try {
            const result =
                method === "password"
                    ? await signozApi.signIn(url.trim(), email.trim(), password, orgId || undefined)
                    : await signozApi.useApiKey(url.trim(), useKeychain ? undefined : apiKey.trim(), useKeychain ? account.trim() : undefined);
            setPassword("");
            setApiKey("");
            if (result.ok) onSignedIn();
            else setError(result.message ?? "SigNoz did not answer the first query");
        } catch (failure) {
            setError(failureMessage(failure));
        } finally {
            setBusy(false);
        }
    };

    const onEnter = (event: React.KeyboardEvent) => {
        if (event.key === "Enter") void submit();
    };

    return (
        <SignInScreen
            mark={<SignozIcon size={28} />}
            title="Connect SigNoz"
            lede="Logs, traces and dashboards from your SigNoz, beside the code that wrote them."
            foot={
                method === "password" && !ssoOnly
                    ? "Your password goes to SigNoz once. Sikemux keeps only the session, in the macOS Keychain."
                    : undefined
            }>
            <div className="signin-form">
                {editingUrl ? (
                    <label className="signin-field">
                        <span>SigNoz URL</span>
                        <input
                            className="signin-input mono"
                            type="url"
                            placeholder="https://signoz.example.com"
                            value={url}
                            onChange={(event) => setUrl(event.target.value)}
                            onBlur={() => look(!!email.trim(), true)}
                            onKeyDown={(event) => {
                                if (event.key === "Enter") look(!!email.trim(), true);
                            }}
                            autoFocus={!status.url}
                            spellCheck={false}
                            autoCapitalize="off"
                            autoCorrect="off"
                        />
                    </label>
                ) : (
                    <div className="signin-address">
                        <span className="signin-address-host">{hostOf(url)}</span>
                        {inspection?.version && <span className="signin-address-meta">SigNoz {inspection.version}</span>}
                        <button type="button" className="signin-link" onClick={() => setEditingUrl(true)}>
                            change
                        </button>
                    </div>
                )}

                {status.keyFromEnvironment && <div className="signin-callout">SIGNOZ_API_KEY is set in your shell, so Sikemux uses that key.</div>}

                {method === "password" ? (
                    <>
                        <label className="signin-field">
                            <span>Email</span>
                            <input
                                className="signin-input"
                                type="email"
                                value={email}
                                onChange={(event) => setEmail(event.target.value)}
                                onBlur={() => look(true)}
                                placeholder="you@example.com"
                                spellCheck={false}
                                autoCapitalize="off"
                                autoCorrect="off"
                            />
                            {inspection?.accountExists === false && <small className="signin-warn">SigNoz does not know this email.</small>}
                        </label>
                        {passwordOrgs.length > 1 && (
                            <label className="signin-field">
                                <span>Organisation</span>
                                <select className="signin-input" value={orgId} onChange={(event) => setOrgId(event.target.value)}>
                                    <option value="">Choose one</option>
                                    {passwordOrgs.map((org) => (
                                        <option key={org.id} value={org.id}>
                                            {org.name || org.id}
                                        </option>
                                    ))}
                                </select>
                            </label>
                        )}
                        {ssoOnly ? (
                            <div className="signin-callout" data-tone="warn">
                                This organisation signs in through{" "}
                                {inspection!.orgs.flatMap((org) => org.sso.map((sso) => sso.provider)).join(" or ")}. Sikemux cannot do that yet, so
                                use an API key.
                            </div>
                        ) : (
                            <label className="signin-field">
                                <span>Password</span>
                                <input
                                    className="signin-input"
                                    type="password"
                                    value={password}
                                    onChange={(event) => setPassword(event.target.value)}
                                    onKeyDown={onEnter}
                                    autoFocus={!!status.url && !!status.email}
                                />
                            </label>
                        )}
                    </>
                ) : (
                    <>
                        {useKeychain ? (
                            <label className="signin-field">
                                <span>Keychain account</span>
                                <input
                                    className="signin-input mono"
                                    value={account}
                                    onChange={(event) => setAccount(event.target.value)}
                                    onKeyDown={onEnter}
                                    spellCheck={false}
                                />
                                <small className="signin-hint">
                                    The account your signoz CLI saved its key under, in the signoz-api Keychain item.
                                </small>
                            </label>
                        ) : (
                            <label className="signin-field">
                                <span>API key</span>
                                <input
                                    className="signin-input mono"
                                    type="password"
                                    value={apiKey}
                                    onChange={(event) => setApiKey(event.target.value)}
                                    onKeyDown={onEnter}
                                />
                                <small className="signin-hint">In SigNoz: Settings → Service Accounts → a service account → Keys.</small>
                            </label>
                        )}
                        <Checkbox checked={useKeychain} onChange={setUseKeychain}>
                            The Keychain already has a key for this SigNoz
                        </Checkbox>
                    </>
                )}

                <button type="button" className="signin-btn primary" disabled={!canSubmit} aria-busy={busy} onClick={() => void submit()}>
                    {busy ? "Connecting…" : method === "password" ? "Sign in" : "Use key"}
                </button>
            </div>

            {error && (
                <div className="signin-callout" data-tone="danger">
                    {error}
                </div>
            )}

            <div className="signin-alt">
                <button type="button" className="signin-link" onClick={() => setMethod(method === "password" ? "apiKey" : "password")}>
                    {method === "password" ? "Use an API key instead" : "Sign in with email instead"}
                </button>
                {method === "apiKey" && !useKeychain && url.trim() && (
                    <button
                        type="button"
                        className="signin-link"
                        onClick={() => void openUrl(`${url.trim().replace(/\/+$/, "")}/settings`).catch(swallow("open SigNoz settings"))}>
                        Open SigNoz settings
                    </button>
                )}
            </div>
        </SignInScreen>
    );
}
