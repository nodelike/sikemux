import { ApiProblem } from "./api";

/** What to tell the person when nothing more is known about what failed. */
export const TRY_AGAIN = "Something went wrong; try again.";

/** The word typed to confirm deleting the account, in the phone app and at app.sikemux.com alike. */
export const CONFIRM_WORD = "delete";

export function confirmsDeletion(typed: string): boolean {
    return typed.trim().toLowerCase() === CONFIRM_WORD;
}

/** Clerk reports problems as an error with a stable `code`, sometimes inside an `errors` list. */
type Reported = {
    code?: string;
    message?: string;
    longMessage?: string;
    errors?: Reported[];
};

export function errorCode(error: unknown): string | undefined {
    const reported = error as Reported | null | undefined;
    return reported?.errors?.[0]?.code ?? reported?.code;
}

/** Words for the person: Clerk's own long message when it has one, which is written for them. */
export function explain(error: unknown): string {
    const reported = error as Reported | null | undefined;
    const first = reported?.errors?.[0] ?? reported;
    return first?.longMessage ?? first?.message ?? TRY_AGAIN;
}

/**
 * Runs `call`; when the API wants the person to prove who they are again, answers what Clerk's
 * `useReverification` takes as "ask for the first factor", within the minutes the API asked for.
 */
export async function orReverify<T>(call: () => Promise<T>) {
    try {
        return await call();
    } catch (error) {
        if (!(error instanceof ApiProblem) || error.reverifyWithinMinutes === undefined) throw error;
        return {
            clerk_error: {
                type: "forbidden",
                reason: "reverification-error",
                metadata: {
                    reverification: {
                        level: "first_factor",
                        afterMinutes: error.reverifyWithinMinutes,
                    },
                },
            },
        } as const;
    }
}

/** How the person proves it is them again: a code sent to their email, or their password. */
export type Factor = { strategy: "email_code"; to: string; emailAddressId: string } | { strategy: "password" };

/** A password is quickest for an account that has one; anyone else gets a code at their email. */
export function chooseFactor(
    supported: readonly {
        strategy: string;
        emailAddressId?: string;
        safeIdentifier?: string;
    }[],
): Factor | undefined {
    if (supported.some((factor) => factor.strategy === "password")) return { strategy: "password" };
    for (const factor of supported)
        if (factor.strategy === "email_code" && factor.emailAddressId)
            return {
                strategy: "email_code",
                to: factor.safeIdentifier ?? "your email",
                emailAddressId: factor.emailAddressId,
            };
    return undefined;
}

/** Why an emailed code is asked for: a device new to the account, a new account, or a new password. */
export type CodePurpose = "trust" | "verify" | "reset";

/** Where signing in with an email address has got to. */
export type SignInStep =
    { kind: "email" } | { kind: "password" } | { kind: "create" } | { kind: "code"; purpose: CodePurpose } | { kind: "new-password" };

type Answer = Promise<{ error: unknown }>;

/** How Clerk moves on once the person is signed in; a navigate that does nothing keeps them where they are. */
type Finalize = { navigate?: () => undefined };

/** The parts of Clerk's sign-in the flow uses, as @clerk/react and @clerk/expo both shape them. */
interface ClerkSignIn {
    readonly status: string | null;
    create(params: { identifier: string }): Answer;
    password(params: { password: string }): Answer;
    finalize(params?: Finalize): Answer;
    mfa: {
        sendEmailCode(): Answer;
        verifyEmailCode(params: { code: string }): Answer;
    };
    resetPasswordEmailCode: {
        sendCode(): Answer;
        verifyCode(params: { code: string }): Answer;
        submitPassword(params: { password: string }): Answer;
    };
}

interface ClerkSignUp {
    readonly status: string | null;
    password(params: { emailAddress: string; password: string }): Answer;
    finalize(params?: Finalize): Answer;
    verifications: {
        sendEmailCode(): Answer;
        verifyEmailCode(params: { code: string }): Answer;
    };
}

type Flow = {
    signIn: ClerkSignIn;
    signUp: ClerkSignUp;
    /** Moves to the next step, which asks the person for something new. */
    go: (step: SignInStep) => void;
    /** How Clerk moves on once the person is signed in. */
    finalize?: () => Finalize | undefined;
    /** Runs once the person is signed in, such as to take them home. */
    done?: () => void;
    /** Why the screen cannot go on: the account needs a step it does not do, or more than it asks for. */
    unsupported: { signIn: string; signUp: string };
};

function check({ error }: { error: unknown }) {
    if (error) throw error;
}

/**
 * Signing in with an email address and a password or emailed code, the same on the web app and the
 * phone: each step's Clerk calls, throwing Clerk's problem when one refuses, and `go` to the next.
 */
export function signInFlow({ signIn, signUp, go, finalize, done, unsupported }: Flow) {
    const finished = async (who: ClerkSignIn | ClerkSignUp) => {
        check(await who.finalize(finalize?.()));
        done?.();
    };

    /** After a password, a device new to the account confirms with an emailed code. */
    const afterPassword = async () => {
        if (signIn.status === "complete") return finished(signIn);
        if (signIn.status === "needs_client_trust" || signIn.status === "needs_second_factor") {
            check(await signIn.mfa.sendEmailCode());
            return go({ kind: "code", purpose: "trust" });
        }
        throw new Error(unsupported.signIn);
    };

    return {
        async submitEmail(email: string) {
            const { error } = await signIn.create({ identifier: email.trim() });
            if (errorCode(error) === "form_identifier_not_found") return go({ kind: "create" });
            check({ error });
            go({ kind: "password" });
        },
        async submitPassword(password: string) {
            check(await signIn.password({ password }));
            await afterPassword();
        },
        async submitNewAccount(email: string, password: string) {
            check(await signUp.password({ emailAddress: email.trim(), password }));
            check(await signUp.verifications.sendEmailCode());
            go({ kind: "code", purpose: "verify" });
        },
        async submitNewPassword(password: string) {
            check(await signIn.resetPasswordEmailCode.submitPassword({ password }));
            await afterPassword();
        },
        async forgot() {
            check(await signIn.resetPasswordEmailCode.sendCode());
            go({ kind: "code", purpose: "reset" });
        },
        async submitCode(purpose: CodePurpose, code: string) {
            if (purpose === "trust") {
                check(await signIn.mfa.verifyEmailCode({ code }));
                return finished(signIn);
            }
            if (purpose === "reset") {
                check(await signIn.resetPasswordEmailCode.verifyCode({ code }));
                return go({ kind: "new-password" });
            }
            check(await signUp.verifications.verifyEmailCode({ code }));
            if (signUp.status !== "complete") throw new Error(unsupported.signUp);
            await finished(signUp);
        },
    };
}
