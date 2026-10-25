/** Accept only credential-free HTTP(S) links; Rust's `open_url` validates again before opening. */
export function safeWebUrl(raw: string): string | null {
    try {
        const url = new URL(raw);
        if ((url.protocol !== "http:" && url.protocol !== "https:") || url.username || url.password) return null;
        return url.href;
    } catch {
        return null;
    }
}
