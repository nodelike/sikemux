import { formatBytes } from "../../../plugin-api/format";
import type { Volume } from "../api";
import { pruneVolumes, removeVolume } from "../storage";

export function Volumes({ volumes, find, onOpen }: { volumes: Volume[]; find: string; onOpen: (container: string) => void }) {
    const words = find.trim().toLowerCase();
    const shown = words
        ? volumes.filter((volume) =>
              [volume.name, volume.composeProject ?? "", ...volume.usedBy].some((field) => field.toLowerCase().includes(words)),
          )
        : volumes;
    const loose = volumes.filter((volume) => volume.anonymous && volume.usedBy.length === 0);
    const total = volumes.reduce((sum, volume) => sum + (volume.size ?? 0), 0);
    return (
        <section className="docker-section" aria-label="Volumes">
            <div className="docker-toolbar">
                <span className="docker-muted">
                    {volumes.length} volumes · {formatBytes(total)}
                </span>
                <button type="button" className="docker-button docker-push" disabled={loose.length === 0} onClick={() => void pruneVolumes()}>
                    Remove unused anonymous{loose.length > 0 ? ` (${loose.length})` : ""}
                </button>
            </div>
            {shown.length === 0 ? (
                <p className="docker-note">{volumes.length === 0 ? "No volumes. Containers that keep data create them." : "No volumes match."}</p>
            ) : (
                <table className="docker-table" aria-label="Volumes">
                    <thead>
                        <tr>
                            <th>Volume</th>
                            <th className="docker-num">Size</th>
                            <th>Used by</th>
                            <th>Created</th>
                            <th aria-label="Actions" />
                        </tr>
                    </thead>
                    <tbody>
                        {shown.map((volume) => (
                            <tr key={volume.name} aria-label={volume.name}>
                                <td>
                                    <span className="docker-mono" title={volume.name}>
                                        {volume.anonymous ? volume.name.slice(0, 12) : volume.name}
                                    </span>
                                    {volume.composeProject ? <span className="docker-chip">{volume.composeProject}</span> : null}
                                    {volume.anonymous ? <span className="docker-chip">anonymous</span> : null}
                                </td>
                                <td className="docker-num">
                                    {volume.size === null ? <span className="docker-muted">—</span> : formatBytes(volume.size)}
                                </td>
                                <td>
                                    {volume.usedBy.length === 0 ? (
                                        <span className="docker-muted">Nothing</span>
                                    ) : (
                                        volume.usedBy.map((container) => (
                                            <button
                                                key={container}
                                                type="button"
                                                className="docker-link docker-link-inline"
                                                onClick={() => onOpen(container)}>
                                                {container}
                                            </button>
                                        ))
                                    )}
                                </td>
                                <td className="docker-muted">{volume.created ? new Date(volume.created).toLocaleDateString() : "—"}</td>
                                <td className="docker-row-actions">
                                    <button
                                        type="button"
                                        className="docker-link danger"
                                        disabled={volume.usedBy.length > 0}
                                        title={volume.usedBy.length > 0 ? "Remove the containers that use it first" : undefined}
                                        onClick={() => void removeVolume(volume)}>
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
