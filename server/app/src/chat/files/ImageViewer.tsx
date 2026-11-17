import { useEffect, useRef } from "react";
import { externalValue, useExternalValue } from "@shared/lib/externalValue";
import { IconClose, IconDownload } from "@shared/ui/Icons";

interface ShownImage {
  /** The data URL the transcript was drawing. */
  readonly src: string;
  /** What to call it when it is saved. */
  readonly name: string;
  /** The file it came from, when it came from one. */
  readonly path?: string;
}

const shown = externalValue<ShownImage | null>(null);

export const showImage = (image: ShownImage): void => shown.set(image);

const hideImage = (): void => shown.set(null);

/** The picture a transcript was showing a thumbnail of, at the size the window allows. */
export function ImageViewer() {
  const image = useExternalValue(shown);
  return image ? <ImageSheet image={image} /> : null;
}

function ImageSheet({ image }: { image: ShownImage }) {
  const closeRef = useRef<HTMLButtonElement>(null);

  useEffect(() => closeRef.current?.focus(), []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") hideImage();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className="img-scrim" onMouseDown={hideImage}>
      <div
        className="img-sheet"
        role="dialog"
        aria-modal="true"
        aria-label={image.name}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="img-bar">
          <span className="img-name" title={image.path ?? image.name}>
            {image.name}
          </span>
          <a
            href={image.src}
            download={image.name}
            aria-label="Save"
            title="Save"
          >
            <IconDownload size={14} />
          </a>
          <button
            ref={closeRef}
            type="button"
            aria-label="Close"
            title="Close — Esc"
            onClick={hideImage}
          >
            <IconClose size={14} />
          </button>
        </div>
        <img className="img-full" src={image.src} alt={image.name} />
      </div>
    </div>
  );
}
