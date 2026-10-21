import { useRef } from "react";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, ResizeHandle, SkeletonRows, StaleNotice } from "../../../plugin-api/ui";
import { formatBytes } from "../../../plugin-api/format";
import { type DockerStatus } from "../api";
import { usePoll } from "../hooks";
import { docker, dockerContainersR, dockerDiskR, dockerImagesR, dockerStacksR, dockerStatsR, dockerStatusR, dockerVolumesR } from "../resources";
import { updateDockerView, useDockerView, type DockerSection } from "../state";
import { ContainerDetail } from "./ContainerDetail";
import { Containers } from "./Containers";
import { Disk } from "./Disk";
import { DockerMark } from "./DockerMark";
import { Images } from "./Images";
import { LogView } from "./LogView";
import { Stacks } from "./Stacks";
import { Volumes } from "./Volumes";
import { STATS_POLL_MS, StatsView } from "./StatsView";
import "../docker.css";

const SIDE_MIN = 170;
const SIDE_MAX = 380;
const LIST_POLL_MS = 4_000;
const STATUS_POLL_MS = 5_000;

export function DockerPane({ paneId, active }: { paneId: string; active: boolean }) {
    const status = useResourceEnabled(active, dockerStatusR);
    usePoll(active && status.data?.running === false, STATUS_POLL_MS, status.refresh);
    if (status.data === undefined && status.status !== "error") return <SkeletonRows rows={5} label="Looking for Docker" />;
    if (!status.data?.running) return <NotRunning status={status.data} error={status.error} onRetry={() => void status.refresh()} />;
    return <Engine paneId={paneId} active={active} status={status.data} />;
}

function NotRunning({ status, error, onRetry }: { status: DockerStatus | undefined; error: string | undefined; onRetry: () => void }) {
    return (
        <div className="docker-pane docker-pane-empty">
            <EmptyState
                icon={<DockerMark size={28} />}
                title="Docker isn't running"
                message={
                    <>
                        <span>{status?.message ?? error ?? "No Docker engine answered."}</span>
                        {status?.socket ? <span className="docker-mono docker-muted">{status.socket}</span> : null}
                        <span className="docker-muted">This pane picks it up on its own once it starts.</span>
                    </>
                }
                action={{ label: "Try again", onClick: onRetry }}
            />
        </div>
    );
}

const SECTIONS: { value: DockerSection; label: string }[] = [
    { value: "containers", label: "Containers" },
    { value: "stacks", label: "Compose" },
    { value: "images", label: "Images" },
    { value: "volumes", label: "Volumes" },
    { value: "disk", label: "Disk use" },
];

function Engine({ paneId, active, status }: { paneId: string; active: boolean; status: DockerStatus }) {
    const view = useDockerView(paneId);
    const sideRef = useRef<HTMLDivElement>(null);
    const containers = useResourceEnabled(active, dockerContainersR);
    usePoll(active, LIST_POLL_MS, containers.refresh);
    const listing = active && view.section === "containers" && !view.container;
    const stats = useResourceEnabled(listing, dockerStatsR);
    usePoll(listing, STATS_POLL_MS, stats.refresh);
    const stacking = active && view.section === "stacks" && !view.container;
    const stacks = useResourceEnabled(active, dockerStacksR);
    usePoll(stacking, LIST_POLL_MS, stacks.refresh);
    const images = useResourceEnabled(active, dockerImagesR);
    const volumes = useResourceEnabled(active, dockerVolumesR);
    const disk = useResourceEnabled(active && view.section === "disk", dockerDiskR);
    const engine = status.engine;
    const running = containers.data?.filter((each) => each.state === "running").length ?? engine?.running ?? 0;
    const total = containers.data?.length ?? engine?.containers ?? 0;
    const counts: Record<DockerSection, string> = {
        containers: `${running}/${total}`,
        stacks: stacks.data ? String(stacks.data.length) : "",
        images: images.data ? String(images.data.length) : "",
        volumes: volumes.data ? String(volumes.data.length) : "",
        disk: disk.data ? formatBytes(disk.data.images.size + disk.data.containers.size + disk.data.volumes.size + disk.data.buildCache.size) : "",
    };

    return (
        <div className="docker-pane">
            <aside className="docker-side" ref={sideRef} style={view.sideWidth ? { width: view.sideWidth } : undefined}>
                <div className="docker-engine">
                    <span className="docker-engine-mark">
                        <DockerMark size={14} />
                    </span>
                    <span className="docker-engine-name">{engine?.name || "Docker"}</span>
                    <span className="docker-muted">{engine ? `v${engine.version}` : ""}</span>
                </div>
                <input
                    className="docker-find"
                    value={view.find}
                    onChange={(event) => updateDockerView(paneId, { find: event.target.value })}
                    onKeyDown={(event) => {
                        if (event.key === "Escape") updateDockerView(paneId, { find: "" });
                    }}
                    placeholder="Find…"
                    aria-label="Find"
                    spellCheck={false}
                />
                <nav className="docker-nav" aria-label="Docker">
                    {SECTIONS.map((section) => (
                        <button
                            key={section.value}
                            type="button"
                            className={`docker-nav-item${view.section === section.value ? " active" : ""}`}
                            aria-current={view.section === section.value ? "page" : undefined}
                            onClick={() => updateDockerView(paneId, { section: section.value, container: null })}>
                            <span>{section.label}</span>
                            <span className="docker-count">{counts[section.value]}</span>
                        </button>
                    ))}
                </nav>
                <div className="docker-side-foot" title={status.socket ?? undefined}>
                    <span className="docker-side-source">{status.source ?? "Docker"}</span>
                    {engine ? (
                        <span className="docker-muted">
                            {engine.cpus} CPU · {formatBytes(engine.memory)}
                        </span>
                    ) : null}
                </div>
            </aside>
            <ResizeHandle
                targetRef={sideRef}
                axis="x"
                grows={1}
                min={SIDE_MIN}
                max={() => SIDE_MAX}
                size={view.sideWidth}
                label="Resize the Docker sidebar"
                className="docker-resize"
                onResize={(size) => {
                    if (size === null && sideRef.current) sideRef.current.style.width = "";
                    updateDockerView(paneId, { sideWidth: size });
                }}
            />
            <main className="docker-main">
                <header className="docker-topbar">
                    <span className="docker-topbar-title">{SECTIONS.find((each) => each.value === view.section)?.label}</span>
                    <button type="button" className="docker-button" onClick={() => docker.refresh()}>
                        Refresh
                    </button>
                </header>
                <div className="docker-page">
                    <StaleNotice handle={containers} />
                    {containers.status === "error" && !containers.data ? (
                        <p className="docker-note">{containers.error}</p>
                    ) : !containers.data ? (
                        <SkeletonRows rows={6} label="Loading containers" />
                    ) : view.container ? (
                        <ContainerDetail
                            active={active}
                            name={view.container}
                            tab={view.tab}
                            onTab={(tab) => updateDockerView(paneId, { tab })}
                            onBack={() => updateDockerView(paneId, { container: null })}>
                            {(detail) => (view.tab === "logs" ? <LogView container={detail} /> : <StatsView active={active} container={detail} />)}
                        </ContainerDetail>
                    ) : view.section === "disk" ? (
                        disk.data ? (
                            <Disk disk={disk.data} onSection={(section) => updateDockerView(paneId, { section, container: null })} />
                        ) : disk.status === "error" ? (
                            <p className="docker-note">{disk.error}</p>
                        ) : (
                            <SkeletonRows rows={4} label="Measuring disk use" />
                        )
                    ) : view.section === "volumes" ? (
                        volumes.data ? (
                            <Volumes volumes={volumes.data} find={view.find} onOpen={(container) => updateDockerView(paneId, { container })} />
                        ) : (
                            <SkeletonRows rows={4} label="Loading volumes" />
                        )
                    ) : view.section === "images" ? (
                        images.data ? (
                            <Images images={images.data} find={view.find} />
                        ) : (
                            <SkeletonRows rows={5} label="Loading images" />
                        )
                    ) : view.section === "stacks" ? (
                        stacks.data ? (
                            <Stacks stacks={stacks.data} find={view.find} onOpen={(container) => updateDockerView(paneId, { container })} />
                        ) : (
                            <SkeletonRows rows={4} label="Loading Compose stacks" />
                        )
                    ) : (
                        <Containers
                            containers={containers.data}
                            stats={stats.data}
                            find={view.find}
                            filter={view.stateFilter}
                            onFilter={(stateFilter) => updateDockerView(paneId, { stateFilter })}
                            onOpen={(container, tab) => updateDockerView(paneId, tab ? { container, tab } : { container })}
                        />
                    )}
                </div>
            </main>
        </div>
    );
}
