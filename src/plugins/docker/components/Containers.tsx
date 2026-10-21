import { useState } from "react";
import { ContextMenu, Seg } from "../../../plugin-api/ui";
import { containerMenu } from "../actions";
import { formatAgo } from "../../../plugin-api/format";
import { formatBytes } from "../../../plugin-api/format";
import type { Container, ContainerStats } from "../api";
import { isRunning, shortImage } from "../format";
import type { ContainerTab, StateFilter } from "../state";
import { Ports, StateDot } from "./parts";

export function matches(container: Container, find: string, filter: StateFilter): boolean {
    if (filter === "running" && !isRunning(container)) return false;
    if (filter === "stopped" && isRunning(container)) return false;
    const words = find.trim().toLowerCase();
    if (!words) return true;
    return [container.name, container.image, container.compose?.project ?? "", container.compose?.service ?? ""].some((field) =>
        field.toLowerCase().includes(words),
    );
}

export function Containers({
    containers,
    stats,
    find,
    filter,
    onFilter,
    onOpen,
}: {
    containers: Container[];
    /** The running containers' load, once it has been measured. */
    stats?: ContainerStats[];
    find: string;
    filter: StateFilter;
    onFilter: (filter: StateFilter) => void;
    onOpen: (name: string, tab?: ContainerTab) => void;
}) {
    const [menu, setMenu] = useState<{ x: number; y: number; container: Container } | null>(null);
    const shown = containers.filter((container) => matches(container, find, filter));
    const found = containers.filter((container) => matches(container, find, "all"));
    const running = found.filter(isRunning).length;
    const filters = [
        { value: "all" as const, label: "All", count: found.length },
        { value: "running" as const, label: "Running", count: running },
        { value: "stopped" as const, label: "Stopped", count: found.length - running },
    ];
    return (
        <section className="docker-section" aria-label="Containers">
            <div className="docker-toolbar">
                <Seg label="Show" options={filters} value={filter} onChange={onFilter} />
            </div>
            {shown.length === 0 ? (
                <p className="docker-note">{containers.length === 0 ? "No containers on this engine yet." : "No containers match."}</p>
            ) : (
                <table className="docker-table" aria-label="Containers">
                    <thead>
                        <tr>
                            <th aria-label="State" />
                            <th>Name</th>
                            <th>Image</th>
                            <th>Status</th>
                            <th>Ports</th>
                            <th className="docker-num">CPU</th>
                            <th className="docker-num">Memory</th>
                            <th>Created</th>
                        </tr>
                    </thead>
                    <tbody>
                        {shown.map((container) => (
                            <tr
                                key={container.id}
                                className="docker-row"
                                tabIndex={0}
                                aria-label={container.name}
                                onClick={() => onOpen(container.name)}
                                onContextMenu={(event) => {
                                    event.preventDefault();
                                    setMenu({ x: event.clientX, y: event.clientY, container });
                                }}
                                onKeyDown={(event) => {
                                    if (event.key === "Enter") onOpen(container.name);
                                }}>
                                <td className="docker-cell-dot">
                                    <StateDot container={container} />
                                </td>
                                <td>
                                    <span className="docker-name">{container.name}</span>
                                    {container.compose ? (
                                        <span className="docker-chip" title={`Compose project ${container.compose.project}`}>
                                            {container.compose.project} · {container.compose.service}
                                        </span>
                                    ) : null}
                                </td>
                                <td className="docker-mono" title={container.image}>
                                    {shortImage(container.image)}
                                </td>
                                <td className="docker-muted">{container.status}</td>
                                <td>
                                    <Ports ports={container.ports} />
                                </td>
                                <Load stats={stats?.find((each) => each.id === container.id)} />
                                <td className="docker-muted">{formatAgo(container.created * 1000)}</td>
                            </tr>
                        ))}
                    </tbody>
                </table>
            )}
            {menu ? (
                <ContextMenu
                    x={menu.x}
                    y={menu.y}
                    items={containerMenu(menu.container, (tab) => onOpen(menu.container.name, tab))}
                    onClose={() => setMenu(null)}
                />
            ) : null}
        </section>
    );
}

function Load({ stats }: { stats: ContainerStats | undefined }) {
    if (!stats) {
        return (
            <>
                <td className="docker-num docker-muted">—</td>
                <td className="docker-num docker-muted">—</td>
            </>
        );
    }
    return (
        <>
            <td className="docker-num">{stats.cpuPercent.toFixed(1)}%</td>
            <td className="docker-num">{formatBytes(stats.memory)}</td>
        </>
    );
}
