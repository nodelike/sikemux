/** A shared chat's link: which share, and the key that reads it. The key never reaches a server. */
export interface ShareLink {
    readonly id: string;
    readonly key: string;
}

const ID = /^[A-Za-z0-9_-]{22}$/;
const KEY = /^[A-Za-z0-9_-]{43}$/;

/**
 * Reads a share link from the web app, `https://app.sikemux.com/s/<id>#<key>`, or from an app,
 * `sikemux://share/<id>#<key>` and its dev build's scheme. Anything else is null.
 */
export function parseShareLink(text: string): ShareLink | null {
    let url: URL;
    try {
        url = new URL(text.trim());
    } catch {
        return null;
    }
    const key = url.hash.slice(1);
    const app = url.protocol === "sikemux:" || url.protocol === "sikemux-dev:";
    const path = app ? `${url.host}${url.pathname}` : url.pathname;
    const match = (app ? /^share\/([^/]+)\/?$/ : /^\/s\/([^/]+)\/?$/).exec(path);
    const id = match?.[1];
    if (!id || !ID.test(id) || !KEY.test(key)) return null;
    return { id, key };
}

/** The link an installed app opens a share from. */
export function appShareLink(link: ShareLink, scheme = "sikemux"): string {
    return `${scheme}://share/${link.id}#${link.key}`;
}
