import { ApiProblem } from "./api.ts";

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
    if (
      !(error instanceof ApiProblem) ||
      error.reverifyWithinMinutes === undefined
    )
      throw error;
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
export type Factor =
  | { strategy: "email_code"; to: string; emailAddressId: string }
  | { strategy: "password" };

/** A password is quickest for an account that has one; anyone else gets a code at their email. */
export function chooseFactor(
  supported: readonly {
    strategy: string;
    emailAddressId?: string;
    safeIdentifier?: string;
  }[],
): Factor | undefined {
  if (supported.some((factor) => factor.strategy === "password"))
    return { strategy: "password" };
  for (const factor of supported)
    if (factor.strategy === "email_code" && factor.emailAddressId)
      return {
        strategy: "email_code",
        to: factor.safeIdentifier ?? "your email",
        emailAddressId: factor.emailAddressId,
      };
  return undefined;
}
