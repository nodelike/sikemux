import { useEffect, useState } from "react";
import { Markdown } from "../../plugin-api/ui";
import { failureMessage } from "../api";
import { useHost } from "../registry";

type Picture = { url: string; data: string | null; failed: string | null };

function usePicture(url: string): Picture | null {
    const { api } = useHost();
    const [loaded, setLoaded] = useState<Picture | null>(null);
    useEffect(() => {
        let alive = true;
        api.image(url)
            .then((data) => {
                if (alive) setLoaded({ url, data, failed: null });
            })
            .catch((error: unknown) => {
                if (alive) setLoaded({ url, data: null, failed: failureMessage(error) });
            });
        return () => {
            alive = false;
        };
    }, [api, url]);
    return loaded?.url === url ? loaded : null;
}

/** An empty circle of the same size holds the place until the picture arrives, so nothing beside it moves. */
export function Avatar({ url, login = null }: { url: string; login?: string | null }) {
    const picture = usePicture(url);
    if (picture?.data) return <img className="gha-avatar" src={picture.data} alt="" width={16} height={16} />;
    if (picture?.failed && login) return <Initial login={login} />;
    return <span className="gha-avatar" aria-hidden="true" title={picture?.failed ? `No picture: ${picture.failed}` : undefined} />;
}

/** Someone with no picture to show, drawn as the first letter of their name. */
export function Initial({ login }: { login: string | null }) {
    return (
        <span className="gha-avatar gha-avatar-letter" aria-hidden="true" title={login ?? undefined}>
            {login?.charAt(0).toUpperCase()}
        </span>
    );
}

export function Prose({ children, className }: { children: string; className?: string }) {
    const { api } = useHost();
    return (
        <Markdown className={className} loadImage={api.image}>
            {children}
        </Markdown>
    );
}
