import { basename } from "../lib/paths";
import { showImage } from "../state/imageViewer";

/* Every picture in a transcript is a thumbnail of itself: it opens at the size
   the window allows, where it can also be saved. */
export function ChatImage({
    src,
    path,
    name = path ? basename(path) : "image.png",
    className = "chat-image",
}: {
    src: string;
    path?: string;
    name?: string;
    className?: string;
}) {
    return (
        <button type="button" className="chat-image-button" title={path ?? name} onClick={() => showImage({ src, name, path })}>
            <img className={className} alt={name} src={src} />
        </button>
    );
}
