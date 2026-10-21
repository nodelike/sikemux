import { useEffect, useRef, useState, type ReactNode } from "react";
import { errorText } from "../lib/format";
import { SignInCancelled, type BrowserSignIn } from "./backend";
import { ScreenField } from "../ui/ShaderField";
import "./signin.css";

/** How a plugin's status says the sign-in it holds stopped working. */
export interface SignInStatus {
    readonly authFailed: boolean;
    readonly message: string | null;
}

export interface SignInAttempt {
    readonly busy: boolean;
    /** Why the last attempt failed, or why the service stopped taking the sign-in the form opened over. */
    readonly error: string | null;
    /** A sign-in in the browser is open. */
    readonly waiting: boolean;
    showError(error: string | null): void;
    /** Runs one sign-in at a time. A message `attempt` resolves with is why the service turned it down. */
    run(attempt: () => Promise<string | null | void>): void;
    /** Follows a sign-in in the browser until it comes back. A message `done` returns is why it was turned down. */
    follow<Result>(signIn: BrowserSignIn<Result>, done: (result: Result) => string | null | void): void;
    cancel(): void;
}

export function useSignInAttempt(status?: SignInStatus | null): SignInAttempt {
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(status?.authFailed ? status.message : null);
    const [waiting, setWaiting] = useState(false);
    const running = useRef(false);
    const open = useRef<BrowserSignIn<unknown> | null>(null);

    useEffect(() => () => open.current?.cancel(), []);

    return {
        busy,
        error,
        waiting,
        showError: setError,
        run(attempt) {
            if (running.current) return;
            running.current = true;
            setBusy(true);
            setError(null);
            void attempt()
                .then(
                    (refused) => refused && setError(refused),
                    (failure: unknown) => setError(errorText(failure)),
                )
                .finally(() => {
                    running.current = false;
                    setBusy(false);
                });
        },
        follow(signIn, done) {
            open.current?.cancel();
            open.current = signIn as BrowserSignIn<unknown>;
            setWaiting(true);
            setError(null);
            const settle = () => {
                if (open.current !== signIn) return false;
                open.current = null;
                setWaiting(false);
                return true;
            };
            signIn.done.then(
                (result) => {
                    if (!settle()) return;
                    const refused = done(result);
                    if (refused) setError(refused);
                },
                (failure: unknown) => {
                    if (settle() && !(failure instanceof SignInCancelled)) setError(errorText(failure));
                },
            );
        },
        cancel: () => open.current?.cancel(),
    };
}

/** The error under a sign-in form, when there is one. */
export function SignInError({ error }: { error: string | null }) {
    if (!error) return null;
    return (
        <div className="signin-callout" data-tone="danger">
            {error}
        </div>
    );
}

interface SignInScreenProps {
    mark: ReactNode;
    title: string;
    lede?: ReactNode;
    foot?: ReactNode;
    children: ReactNode;
}

/** One column in the middle of the pane: the service's mark, what signing in gives, one button, and the quieter ways round below. */
export function SignInScreen({ mark, title, lede, foot, children }: SignInScreenProps) {
    return (
        <div className="signin-screen">
            <ScreenField />
            <div className="signin">
                <span className="signin-mark">{mark}</span>
                <h2 className="signin-title">{title}</h2>
                {lede && <p className="signin-lede">{lede}</p>}
                {children}
                {foot && <p className="signin-foot">{foot}</p>}
            </div>
        </div>
    );
}

/** Stands in for the main button while the person finishes signing in somewhere else. */
export function SignInWaiting({ children, onCancel }: { children: ReactNode; onCancel: () => void }) {
    return (
        <div className="signin-waiting" role="status">
            <span className="signin-spinner" aria-hidden="true" />
            {children}
            <button type="button" className="signin-link" onClick={onCancel}>
                Cancel
            </button>
        </div>
    );
}
