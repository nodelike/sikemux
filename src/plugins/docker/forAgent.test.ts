import { describe, expect, it } from "vitest";
import type { ContainerDetail } from "./api";
import { containerForAgent } from "./forAgent";

const worker = {
    id: "1e7f0c4b9d62",
    name: "shop-worker-1",
    image: "docker.io/library/shop-api:dev",
    state: { status: "exited", running: false, exitCode: 1, error: null, oomKilled: false, startedAt: null, finishedAt: null, health: null },
    restartCount: 3,
    restartPolicy: "unless-stopped",
    env: ["DATABASE_URL"],
    ports: [],
    compose: { project: "shop", service: "worker", number: 1, workingDir: "/Users/me/shop", configFiles: [] },
} as unknown as ContainerDetail;

describe("a container handed to an agent", () => {
    it("says how it ended, where it comes from, and the end of its log with stderr marked", () => {
        const text = containerForAgent(worker, [
            { at: null, stream: "stdout", text: "starting" },
            { at: null, stream: "stderr", text: "password authentication failed" },
        ]);
        expect(text).toContain("Docker container shop-worker-1 (1e7f0c4b9d62)");
        expect(text).toContain("Image: shop-api:dev");
        expect(text).toContain("Compose: service worker of shop in /Users/me/shop");
        expect(text).toContain("State: exited with code 1");
        expect(text).toContain("Restarts: 3 (policy unless-stopped)");
        expect(text).toContain("starting\n[err] password authentication failed");
        expect(text).toContain("use docker_logs or docker_container with id shop-worker-1");
    });

    it("leaves out the log when there is none", () => {
        expect(containerForAgent(worker, [])).not.toContain("End of its log");
    });
});
