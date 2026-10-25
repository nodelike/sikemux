import type { ShareLink } from "./link";

/** Unseals what the host sealed for a share's viewers, and seals what they ask it. */
export interface ShareCipher {
    /** The text inside, or null when it was not sealed with this share's key. */
    open(sealed: string): Promise<string | null>;
    seal(text: string): Promise<string>;
    /** What shows the server this viewer holds the link, without giving it the key. */
    viewerProof(): Promise<string>;
}

export const NONCE_BYTES = 12;

/** What a share's proof is an HMAC of, keyed with the link's key; `server/protocol/vectors/share-viewer.json` pins it. */
export function viewerProofText(id: string): string {
    return `sikemux-share-viewer|v1|${id}`;
}

/** Binds every sealed text to its share, as the host does. */
export function shareAad(id: string): string {
    return `sikemux-share|v1|${id}`;
}

export function fromBase64Url(text: string): Uint8Array<ArrayBuffer> {
    const base64 = text.replace(/-/g, "+").replace(/_/g, "/");
    const binary = atob(base64 + "=".repeat((4 - (base64.length % 4)) % 4));
    const bytes = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
    return bytes;
}

export function toBase64Url(bytes: Uint8Array): string {
    let binary = "";
    for (let index = 0; index < bytes.length; index += 0x8000) binary += String.fromCharCode(...bytes.subarray(index, index + 0x8000));
    return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** AES-256-GCM through the browser's own crypto, for the Mac and the web app. */
export function webCipher(link: ShareLink): ShareCipher {
    const aad = new TextEncoder().encode(shareAad(link.id));
    const key = crypto.subtle.importKey("raw", fromBase64Url(link.key), "AES-GCM", false, ["encrypt", "decrypt"]);
    return {
        async open(sealed) {
            try {
                const bytes = fromBase64Url(sealed);
                const iv = bytes.subarray(0, NONCE_BYTES);
                const plain = await crypto.subtle.decrypt({ name: "AES-GCM", iv, additionalData: aad }, await key, bytes.subarray(NONCE_BYTES));
                return new TextDecoder().decode(plain);
            } catch {
                return null;
            }
        },
        async seal(text) {
            const iv = crypto.getRandomValues(new Uint8Array(NONCE_BYTES));
            const sealed = await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: aad }, await key, new TextEncoder().encode(text));
            const out = new Uint8Array(NONCE_BYTES + sealed.byteLength);
            out.set(iv);
            out.set(new Uint8Array(sealed), NONCE_BYTES);
            return toBase64Url(out);
        },
        async viewerProof() {
            const hmac = await crypto.subtle.importKey("raw", fromBase64Url(link.key), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
            const proof = await crypto.subtle.sign("HMAC", hmac, new TextEncoder().encode(viewerProofText(link.id)));
            return toBase64Url(new Uint8Array(proof));
        },
    };
}
