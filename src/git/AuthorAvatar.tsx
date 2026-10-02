import { createContext, useContext, useEffect, useState } from "react";

export function initials(name: string): string {
    const parts = name.trim().split(/\s+/).filter(Boolean);
    if (parts.length === 0) return "?";
    if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
    return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
}

export function authorColor(key: string): string {
    let h = 0;
    for (let i = 0; i < key.length; i++) h = (h * 31 + key.charCodeAt(i)) >>> 0;
    return `hsl(${h % 360} 62% 64%)`;
}

/** Where a commit author's picture comes from: the code host the repository is on, when there is one. */
export interface AuthorPictures {
    /** The picture's address for a commit email, or null when the host knows no account by it. */
    pictureFor(email: string): string | null;
    /** The picture itself, as an address the window may show. */
    load(url: string): Promise<string>;
}

const AuthorPicturesContext = createContext<AuthorPictures | null>(null);

export const AuthorPicturesProvider = AuthorPicturesContext.Provider;

/** Pictures already fetched, so a history that scrolls rows back into view does not fetch them again. */
const loaded = new Map<string, string>();

/** A commit author's picture from the code host, or their initials on a colour of their own until then or instead. */
export function AuthorAvatar({ name, email }: { name: string; email: string }) {
    const pictures = useContext(AuthorPicturesContext);
    const url = pictures && email ? pictures.pictureFor(email) : null;
    const [data, setData] = useState<{ url: string; data: string } | null>(() => {
        const known = url ? loaded.get(url) : undefined;
        return url && known ? { url, data: known } : null;
    });
    useEffect(() => {
        if (!url || !pictures) return;
        const known = loaded.get(url);
        if (known) {
            setData({ url, data: known });
            return;
        }
        let alive = true;
        pictures
            .load(url)
            .then((picture) => {
                loaded.set(url, picture);
                if (alive) setData({ url, data: picture });
            })
            .catch(() => {});
        return () => {
            alive = false;
        };
    }, [url, pictures]);

    if (url && data?.url === url) return <img className="gg-avatar gg-avatar-picture" src={data.data} alt="" aria-hidden="true" />;
    return (
        <span className="gg-avatar" style={{ background: authorColor(email || name) }} aria-hidden="true">
            {initials(name)}
        </span>
    );
}
