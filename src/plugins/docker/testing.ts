import type { Container, DockerStatus } from "./api";

export const engineRunning: DockerStatus = {
    running: true,
    message: null,
    socket: "/Users/me/.colima/default/docker.sock",
    source: "colima",
    engine: {
        name: "colima",
        version: "29.2.1",
        apiVersion: "1.53",
        os: "Ubuntu 24.04",
        arch: "aarch64",
        cpus: 4,
        memory: 6_000_000_000,
        containers: 3,
        running: 2,
        paused: 0,
        stopped: 1,
        images: 9,
    },
};

export const engineStopped: DockerStatus = {
    running: false,
    message: "Docker isn't running. Start Docker Desktop, Colima or OrbStack, then try again.",
    socket: null,
    source: null,
    engine: null,
};

export function container(name: string, patch: Partial<Container> = {}): Container {
    return {
        id: `${name}-id`.padEnd(12, "0").slice(0, 12),
        name,
        image: "nginx:alpine",
        command: "nginx",
        created: 1_792_000_000,
        state: "running",
        status: "Up 2 hours",
        ports: [],
        compose: null,
        ...patch,
    };
}

export const SHOP_WEB = container("shop-web-1", {
    image: "shop-web",
    ports: [{ private: 3000, public: 3000, ip: "0.0.0.0", protocol: "tcp" }],
    compose: { project: "shop", service: "web", number: 1, workingDir: "/Users/me/shop", configFiles: ["/Users/me/shop/compose.yaml"] },
});
export const SHOP_DB = container("shop-db-1", {
    image: "postgres:16",
    state: "exited",
    status: "Exited (1) 3 minutes ago",
    compose: { project: "shop", service: "db", number: 1, workingDir: "/Users/me/shop", configFiles: ["/Users/me/shop/compose.yaml"] },
});
export const REDIS = container("redis", { image: "redis:7" });
