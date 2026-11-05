const BASE_MS = 1000;
const MAX_MS = 60_000;
/** The server's limits count over a minute, so a client over one waits at least this long. */
export const OVER_LIMIT_WAIT_MS = 30_000;
/** A sign-in is renewed this long before the server would close the connection for it. */
const RENEW_AHEAD_MS = 15_000;

/** Exponential backoff with half of it random, so clients that lost the server together do not return together. */
export function reconnectDelay(attempt: number, random: number): number {
    const ceiling = Math.min(MAX_MS, BASE_MS * 2 ** attempt);
    return Math.round(ceiling / 2 + (ceiling / 2) * random);
}

/** When a Clerk session token stops being accepted, in milliseconds since 1970, or 0 when it cannot be read. */
function tokenExpiry(token: string): number {
    try {
        const payload = (token.split(".")[1] ?? "").replace(/-/g, "+").replace(/_/g, "/");
        const exp = (JSON.parse(atob(payload)) as { exp?: unknown }).exp;
        return typeof exp === "number" ? exp * 1000 : 0;
    } catch {
        return 0;
    }
}

/**
 * Keeps a socket's sign-in ahead of when the server would close it. Clerk may still hand out the token the
 * server already has; it is asked past its cache then.
 */
export class SignInRenewal {
    private timer: ReturnType<typeof setTimeout> | undefined;
    private readonly token: (fresh: boolean) => Promise<string | null>;

    constructor(token: (fresh: boolean) => Promise<string | null>) {
        this.token = token;
    }

    /** Sends a fresh token with `send` before `expiry`, and keeps doing so unless `send` says the socket is gone. */
    before(expiry: number, send: (token: string) => boolean): void {
        this.stop();
        this.timer = setTimeout(() => void this.renew(expiry, send), Math.max(1000, expiry - Date.now() - RENEW_AHEAD_MS));
    }

    stop(): void {
        clearTimeout(this.timer);
        this.timer = undefined;
    }

    private async renew(expiry: number, send: (token: string) => boolean): Promise<void> {
        let token = await this.token(false);
        if (token && tokenExpiry(token) <= expiry) token = await this.token(true);
        if (token && send(token)) this.before(tokenExpiry(token), send);
    }
}
