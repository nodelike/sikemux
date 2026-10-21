import { useState, type ReactNode } from "react";
import type { BrowserSignIn } from "./backend";
import { openSignInUrl, openUrl, swallow } from "./host";
import { SignInError, SignInScreen, SignInWaiting, useSignInAttempt, type SignInStatus } from "./SignIn";

export interface TokenField {
    readonly name: string;
    readonly label: string;
    readonly placeholder: string;
    /** A key or token: typed out of sight, in monospace. */
    readonly secret?: boolean;
    readonly email?: boolean;
    readonly optional?: boolean;
    readonly initial?: string;
    readonly hint?: ReactNode;
    /** Hidden until the person follows a link with this label, such as one for a company's own server. */
    readonly revealedBy?: string;
}

export type TokenValues = Readonly<Record<string, string>>;

export interface BrowserWay<Result> {
    readonly label: string;
    readonly icon: ReactNode;
    readonly start: (openPage: (url: string) => void) => BrowserSignIn<Result>;
    /** A message it returns is why the service turned the account away. */
    readonly done: (result: Result) => string | null | void;
    /** The link to the token form, such as "Use an API key instead". */
    readonly tokenInstead: string;
}

export interface TokenSignInProps<Result> {
    readonly status: SignInStatus | null | undefined;
    /** The service's name, as in "Connect Linear". */
    readonly service: string;
    readonly mark: ReactNode;
    readonly lede: string;
    readonly fields: readonly TokenField[];
    /** Gets every field trimmed. A message it returns is why the service turned the token down. */
    readonly signIn: (values: TokenValues) => Promise<string | null | void>;
    readonly submitLabel?: string;
    readonly makeToken: (values: TokenValues) => { readonly label: string; readonly url: string };
    /** Offered first when there is one; the token form is the way round. */
    readonly browser?: BrowserWay<Result>;
    /** A sign-in already on this machine that can be used as it is, such as a signed-in CLI. */
    readonly found?: {
        readonly label: string;
        readonly icon: ReactNode;
        readonly note: string;
        readonly use: () => Promise<string | null | void>;
    } | null;
}

/** The sign-in form of a service that takes a pasted token, and maybe a sign-in in the browser before it. */
export function TokenSignIn<Result>({
    status,
    service,
    mark,
    lede,
    fields,
    signIn,
    submitLabel = "Sign in",
    makeToken,
    browser,
    found,
}: TokenSignInProps<Result>) {
    const [withToken, setWithToken] = useState(!browser);
    const [values, setValues] = useState<TokenValues>(() => Object.fromEntries(fields.map((field) => [field.name, field.initial ?? ""])));
    const [revealed, setRevealed] = useState<ReadonlySet<string>>(new Set());
    const attempt = useSignInAttempt(status);
    const shown = fields.filter((field) => !field.revealedBy || revealed.has(field.name));
    const trimmed = (): TokenValues => Object.fromEntries(fields.map((field) => [field.name, values[field.name].trim()]));
    const canSubmit = !attempt.busy && shown.every((field) => field.optional || values[field.name].trim());
    const submit = () => {
        if (canSubmit) attempt.run(() => signIn(trimmed()));
    };
    const focused = found ? null : shown.find((field) => !field.optional);
    const tokenPage = makeToken(trimmed());

    return (
        <SignInScreen mark={mark} title={`Connect ${service}`} lede={lede} foot="Sikemux keeps your sign-in in the macOS Keychain.">
            {found && (
                <>
                    <button type="button" className="signin-btn primary" disabled={attempt.busy} onClick={() => attempt.run(found.use)}>
                        {found.icon}
                        {attempt.busy ? "Checking…" : found.label}
                    </button>
                    <p className="signin-note">{found.note}</p>
                    <div className="signin-or">or paste a token</div>
                </>
            )}
            {withToken ? (
                <div className="signin-form">
                    {shown.map((field) => (
                        <label key={field.name} className="signin-field">
                            <span>{field.label}</span>
                            <input
                                className={`signin-input${field.secret ? " mono" : ""}`}
                                type={field.secret ? "password" : field.email ? "email" : "text"}
                                value={values[field.name]}
                                onChange={(event) => setValues({ ...values, [field.name]: event.target.value })}
                                onKeyDown={(event) => {
                                    if (event.key === "Enter") submit();
                                }}
                                placeholder={field.placeholder}
                                autoFocus={field === focused}
                                spellCheck={false}
                                autoCapitalize="off"
                                autoCorrect="off"
                            />
                            {field.hint && <small className="signin-hint">{field.hint}</small>}
                        </label>
                    ))}
                    <button type="button" className={`signin-btn${found ? "" : " primary"}`} disabled={!canSubmit} onClick={submit}>
                        {attempt.busy ? "Checking…" : submitLabel}
                    </button>
                </div>
            ) : attempt.waiting ? (
                <SignInWaiting onCancel={attempt.cancel}>Finish signing in in your browser</SignInWaiting>
            ) : (
                browser && (
                    <button
                        type="button"
                        className="signin-btn primary"
                        onClick={() =>
                            attempt.follow(
                                browser.start((url) => void openSignInUrl(url).catch(swallow("open the sign-in page"))),
                                browser.done,
                            )
                        }>
                        {browser.icon}
                        {browser.label}
                    </button>
                )
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
                        <button
                            type="button"
                            className="signin-link"
                            onClick={() => void openUrl(tokenPage.url).catch(swallow("open the token page"))}>
                            {tokenPage.label}
                        </button>
                        {fields
                            .filter((field) => field.revealedBy && !revealed.has(field.name))
                            .map((field) => (
                                <button
                                    key={field.name}
                                    type="button"
                                    className="signin-link"
                                    onClick={() => setRevealed(new Set([...revealed, field.name]))}>
                                    {field.revealedBy}
                                </button>
                            ))}
                    </>
                ) : (
                    browser && (
                        <button type="button" className="signin-link" disabled={attempt.waiting} onClick={() => setWithToken(true)}>
                            {browser.tokenInstead}
                        </button>
                    )
                )}
            </div>
        </SignInScreen>
    );
}
