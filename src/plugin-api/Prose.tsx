import { createContext, memo, useContext, useEffect, useMemo, useState, type ComponentType, type ReactNode } from "react";
import { Markdown as MarkdownText, MARKDOWN_GFM, type MarkdownComponents } from "../markdown/Markdown";
import { openUrl, swallow } from "./host";
import "./prose.css";

/** Turns a picture's address into one the window may show, such as a `data:` address. */
export type ProseImageLoader = (src: string) => Promise<string>;

/** Only addresses a browser can open; anything stranger is shown as plain text. */
function safeHref(href: string): string | null {
    const trimmed = href.trim();
    return /^https?:\/\//iu.test(trimmed) ? trimmed : null;
}

function ProseLink({ href, children }: { href: string; children: ReactNode }) {
    const target = safeHref(href);
    if (!target) return <>{children}</>;
    return (
        <a
            href={target}
            onClick={(event) => {
                // The webview must not navigate away from the app.
                event.preventDefault();
                void openUrl(target).catch(swallow("open the link"));
            }}>
            {children}
        </a>
    );
}

const ImageLoaderContext = createContext<ProseImageLoader | null>(null);

function ProseImage({ src, alt, title, inLink }: { src: string; alt: string; title?: string; inLink: boolean }) {
    const load = useContext(ImageLoaderContext);
    const [loaded, setLoaded] = useState<{ src: string; url: string } | null>(null);
    useEffect(() => {
        if (!load) return;
        let alive = true;
        load(src)
            .then((url) => {
                if (alive) setLoaded({ src, url });
            })
            .catch(swallow("load a picture"));
        return () => {
            alive = false;
        };
    }, [load, src]);

    if (loaded?.src === src) return <img src={loaded.url} alt={alt} title={title} />;
    const label = alt || src;
    return inLink ? <>{label}</> : <ProseLink href={src}>{label}</ProseLink>;
}

const COMPONENTS: MarkdownComponents = { link: ProseLink };
const WITH_HTML_IMAGES = { ...MARKDOWN_GFM, htmlImages: true };
const WITH_IMAGES: MarkdownComponents = { link: ProseLink, img: ProseImage };

/**
 * Prose somebody else wrote, such as a release's notes or a comment, drawn by
 * the app's own markdown reader with embedded HTML left out and links handed
 * to the browser. Pictures are drawn only through `loadImage`, since the
 * window cannot load them from the web itself; until one arrives, its
 * description stands in as a link to it.
 */
export const Markdown = memo(function Markdown({
    children,
    className = "prose",
    loadImage,
    taskBox,
}: {
    children: string;
    className?: string;
    loadImage?: ProseImageLoader;
    /** Draws a task list item's box, for prose whose tasks can be ticked. */
    taskBox?: ComponentType<{ checked: boolean }>;
}) {
    const base = loadImage ? WITH_IMAGES : COMPONENTS;
    const components = useMemo(() => (taskBox ? { ...base, taskBox } : base), [base, taskBox]);
    return (
        <div className={className}>
            <ImageLoaderContext.Provider value={loadImage ?? null}>
                <MarkdownText text={children} options={loadImage ? WITH_HTML_IMAGES : MARKDOWN_GFM} components={components} />
            </ImageLoaderContext.Provider>
        </div>
    );
});
