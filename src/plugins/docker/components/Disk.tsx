import { docker } from "../resources";
import type { ReactNode } from "react";
import { formatBytes } from "../../../plugin-api/format";
import { runInTerminal, swallow } from "../../../plugin-api/host";
import { type Disk as DiskUsage, type Usage } from "../api";
import { percent } from "../format";
import type { DockerSection } from "../state";
import { pruneImages, pruneVolumes } from "../storage";

export function Disk({ disk, onSection }: { disk: DiskUsage; onSection: (section: DockerSection) => void }) {
    const total = disk.images.size + disk.containers.size + disk.volumes.size + disk.buildCache.size;
    const reclaimable = disk.images.reclaimable + disk.containers.reclaimable + disk.volumes.reclaimable + disk.buildCache.reclaimable;
    return (
        <section className="docker-section" aria-label="Disk">
            <p className="docker-disk-total">
                <strong>{formatBytes(total)}</strong> used by Docker
                {reclaimable > 0 ? <span className="docker-muted"> · {formatBytes(reclaimable)} could be freed</span> : null}
            </p>
            <div className="docker-stats">
                <Card title="Images" usage={disk.images} unit="in use">
                    <button type="button" className="docker-link" onClick={() => onSection("images")}>
                        Show images
                    </button>
                    <button type="button" className="docker-link" disabled={disk.images.reclaimable === 0} onClick={() => void pruneImages()}>
                        Remove untagged
                    </button>
                </Card>
                <Card title="Containers" usage={disk.containers} unit="running">
                    <button type="button" className="docker-link" onClick={() => onSection("containers")}>
                        Show containers
                    </button>
                </Card>
                <Card title="Volumes" usage={disk.volumes} unit="in use">
                    <button type="button" className="docker-link" onClick={() => onSection("volumes")}>
                        Show volumes
                    </button>
                    <button type="button" className="docker-link" disabled={disk.volumes.reclaimable === 0} onClick={() => void pruneVolumes()}>
                        Remove unused anonymous
                    </button>
                </Card>
                <Card title="Build cache" usage={disk.buildCache} unit="in use">
                    <button
                        type="button"
                        className="docker-link"
                        disabled={disk.buildCache.reclaimable === 0}
                        onClick={() =>
                            void runInTerminal("Clear the build cache", ["docker", "builder", "prune"])
                                .then(() => docker.refresh())
                                .catch(swallow("clear the build cache"))
                        }>
                        Clear in a terminal
                    </button>
                </Card>
            </div>
        </section>
    );
}

function Card({ title, usage, unit, children }: { title: string; usage: Usage; unit: string; children: ReactNode }) {
    const share = percent(usage.reclaimable, usage.size);
    return (
        <section className="docker-facts docker-meter" aria-label={title}>
            <h3>{title}</h3>
            <div className="docker-meter-value">
                <strong>{formatBytes(usage.size)}</strong>
                <span className="docker-muted">
                    {usage.count} · {usage.active} {unit}
                </span>
            </div>
            <div
                className="docker-bar"
                role="meter"
                aria-label={`${title} that could be freed`}
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={share}>
                <span style={{ width: `${share}%` }} />
            </div>
            <span className="docker-muted">{usage.reclaimable > 0 ? `${formatBytes(usage.reclaimable)} could be freed` : "Nothing to free"}</span>
            <div className="docker-card-actions">{children}</div>
        </section>
    );
}
