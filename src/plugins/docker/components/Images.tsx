import { formatAgo, formatBytes } from "../../../plugin-api/format";
import { Seg } from "../../../plugin-api/ui";
import type { Image } from "../api";
import { imageName, pruneImages, removeImage, runImage } from "../storage";
import { shortImage } from "../format";
import { useState } from "react";

type ImageFilter = "all" | "used" | "unused" | "dangling";

export function Images({ images, find }: { images: Image[]; find: string }) {
    const [filter, setFilter] = useState<ImageFilter>("all");
    const words = find.trim().toLowerCase();
    const found = words
        ? images.filter((image) => image.tags.some((tag) => tag.toLowerCase().includes(words)) || image.id.startsWith(words))
        : images;
    const shown = found.filter((image) =>
        filter === "used" ? image.containers > 0 : filter === "unused" ? image.containers === 0 : filter === "dangling" ? image.dangling : true,
    );
    const dangling = images.filter((image) => image.dangling && image.containers === 0);
    const total = images.reduce((sum, image) => sum + image.size, 0);
    return (
        <section className="docker-section" aria-label="Images">
            <div className="docker-toolbar">
                <Seg
                    label="Show"
                    value={filter}
                    onChange={setFilter}
                    options={[
                        { value: "all", label: "All", count: found.length },
                        { value: "used", label: "In use", count: found.filter((image) => image.containers > 0).length },
                        { value: "unused", label: "Unused", count: found.filter((image) => image.containers === 0).length },
                        { value: "dangling", label: "Untagged", count: found.filter((image) => image.dangling).length },
                    ]}
                />
                <span className="docker-muted">{formatBytes(total)} in all</span>
                <button type="button" className="docker-button docker-push" disabled={dangling.length === 0} onClick={() => void pruneImages()}>
                    Remove untagged{dangling.length > 0 ? ` (${formatBytes(dangling.reduce((sum, image) => sum + image.size, 0))})` : ""}
                </button>
            </div>
            {shown.length === 0 ? (
                <p className="docker-note">{images.length === 0 ? "No images yet. They show here once pulled or built." : "No images match."}</p>
            ) : (
                <table className="docker-table" aria-label="Images">
                    <thead>
                        <tr>
                            <th>Image</th>
                            <th>ID</th>
                            <th className="docker-num">Size</th>
                            <th>Created</th>
                            <th className="docker-num">Containers</th>
                            <th aria-label="Actions" />
                        </tr>
                    </thead>
                    <tbody>
                        {shown.map((image) => (
                            <tr key={image.id} aria-label={imageName(image)}>
                                <td className="docker-mono">
                                    {image.tags.length > 0 ? image.tags.map(shortImage).join(", ") : <span className="docker-muted">untagged</span>}
                                </td>
                                <td className="docker-mono docker-muted">{image.id}</td>
                                <td className="docker-num">{formatBytes(image.size)}</td>
                                <td className="docker-muted">{formatAgo(image.created * 1000)}</td>
                                <td className="docker-num">{image.containers || <span className="docker-muted">—</span>}</td>
                                <td className="docker-row-actions">
                                    {image.tags.length > 0 ? (
                                        <button type="button" className="docker-link" onClick={() => runImage(image)}>
                                            Run
                                        </button>
                                    ) : null}
                                    <button type="button" className="docker-link danger" onClick={() => void removeImage(image)}>
                                        Remove
                                    </button>
                                </td>
                            </tr>
                        ))}
                    </tbody>
                </table>
            )}
        </section>
    );
}
