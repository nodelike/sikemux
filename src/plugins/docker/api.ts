import { createPluginBackend, type PluginStream, type PluginStreamHandlers } from "../../plugin-api/backend";
import { DOCKER_PLUGIN } from "./kinds";

export { isPluginFailure } from "../../plugin-api/backend";

const backend = createPluginBackend(DOCKER_PLUGIN.id);

export interface EngineInfo {
    name: string;
    version: string;
    apiVersion: string;
    os: string;
    arch: string;
    cpus: number;
    memory: number;
    containers: number;
    running: number;
    paused: number;
    stopped: number;
    images: number;
}

export interface DockerStatus {
    running: boolean;
    /** Why no engine answered, in words for the person. */
    message: string | null;
    socket: string | null;
    /** `DOCKER_HOST`, the context's name, or the folder the socket was found in. */
    source: string | null;
    engine: EngineInfo | null;
}

export interface Port {
    private: number;
    public: number | null;
    ip: string | null;
    protocol: string;
}

export interface Compose {
    project: string;
    service: string;
    number: number | null;
    workingDir: string | null;
    configFiles: string[];
}

/** created, running, paused, restarting, removing, exited or dead. */
export type ContainerState = string;

export interface Container {
    id: string;
    name: string;
    image: string;
    command: string;
    /** Seconds since 1970. */
    created: number;
    state: ContainerState;
    status: string;
    ports: Port[];
    compose: Compose | null;
}

export interface Health {
    status: string;
    failingStreak: number;
    lastOutput: string[];
}

export interface ContainerDetail {
    id: string;
    name: string;
    image: string;
    imageId: string;
    created: string;
    command: string[];
    entrypoint: string[];
    workingDir: string;
    state: {
        status: string;
        running: boolean;
        exitCode: number;
        error: string | null;
        oomKilled: boolean;
        startedAt: string | null;
        finishedAt: string | null;
        health: Health | null;
    };
    restartCount: number;
    restartPolicy: string;
    /** The names of its environment variables. Their values never reach the pane. */
    env: string[];
    mounts: { kind: string; source: string; destination: string; writable: boolean; volume: string | null }[];
    networks: { name: string; ip: string | null; aliases: string[] }[];
    ports: Port[];
    labels: Record<string, string>;
    compose: Compose | null;
}

export type ContainerAction = "start" | "stop" | "restart" | "pause" | "unpause" | "kill";

export interface LogLine {
    at: string | null;
    stream: "stdout" | "stderr";
    text: string;
}

export interface LogBatch {
    lines: LogLine[];
    /** The log ended, as it does when the container stops. */
    ended: boolean;
}

export interface ContainerStats {
    id: string;
    name: string;
    cpuPercent: number;
    memory: number;
    memoryLimit: number;
    netRx: number;
    netTx: number;
    diskRead: number;
    diskWrite: number;
    pids: number;
}

export interface Stack {
    project: string;
    workingDir: string | null;
    configFiles: string[];
    services: { name: string; containers: Container[] }[];
    running: number;
    total: number;
}

export type StackAction = "start" | "stop" | "restart" | "down";

export interface StackResult {
    project: string;
    done: string[];
    failed: { name: string; message: string }[];
}

export interface Image {
    id: string;
    tags: string[];
    created: number;
    size: number;
    containers: number;
    dangling: boolean;
}

export interface Volume {
    name: string;
    driver: string;
    created: string | null;
    size: number | null;
    usedBy: string[];
    anonymous: boolean;
    composeProject: string | null;
}

export interface Usage {
    count: number;
    active: number;
    size: number;
    reclaimable: number;
}

export interface Disk {
    images: Usage;
    containers: Usage;
    volumes: Usage;
    buildCache: Usage;
}

export interface Pruned {
    deleted: number;
    spaceReclaimed: number;
}

export const LOG_TAIL = 1000;

export const dockerApi = {
    status: () => backend.call<DockerStatus>("status"),
    containers: () => backend.call<Container[]>("containers", { all: true }),
    container: (id: string) => backend.call<ContainerDetail>("container", { id }),
    act: (id: string, action: ContainerAction) => backend.call<ContainerDetail>("act", { id, action }),
    remove: (id: string, force: boolean) => backend.call<void>("remove", { id, force, volumes: false }),
    followLogs: (id: string, handlers: PluginStreamHandlers<LogBatch>): PluginStream =>
        backend.stream<LogBatch>("followLogs", { id, tail: LOG_TAIL }, handlers),
    stats: () => backend.call<ContainerStats[]>("stats", {}),
    stacks: () => backend.call<Stack[]>("stacks"),
    actOnStack: (project: string, action: StackAction) => backend.call<StackResult>("actOnStack", { project, action }),
    images: () => backend.call<Image[]>("images", {}),
    removeImage: (id: string, force: boolean) => backend.call<unknown>("removeImage", { id, force }),
    pruneImages: () => backend.call<Pruned>("pruneImages"),
    volumes: () => backend.call<Volume[]>("volumes"),
    removeVolume: (name: string) => backend.call<void>("removeVolume", { name }),
    pruneVolumes: () => backend.call<{ deleted: string[]; spaceReclaimed: number }>("pruneVolumes"),
    disk: () => backend.call<Disk>("disk"),
};
