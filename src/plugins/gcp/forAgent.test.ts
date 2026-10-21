import { describe, expect, it } from "vitest";
import { podForAgent, serialForAgent, serviceForAgent } from "./forAgent";

describe("what agents are handed", () => {
    it("a service names its failure, image, traffic and how to read its logs", () => {
        const text = serviceForAgent("acme-prod", {
            name: "checkout",
            location: "us-central1",
            url: "https://checkout.run.app",
            state: "FAILED",
            message: "the container failed to start",
            latestRevision: "checkout-00030",
            image: "img:0.9.1",
            cpu: null,
            memory: null,
            minInstances: null,
            maxInstances: null,
            concurrency: null,
            serviceAccount: null,
            ingress: null,
            traffic: [{ revision: "checkout-00030", latest: false, percent: 100, tag: null, url: null }],
            updatedAt: null,
            updatedBy: null,
            revisions: [],
            env: [{ name: "DB_URL", secret: "db-url" }],
        });
        expect(text).toContain("Cloud Run service checkout in acme-prod (us-central1): FAILED");
        expect(text).toContain("Status: the container failed to start");
        expect(text).toContain("Traffic: 100% checkout-00030");
        expect(text).toContain("Environment variables: DB_URL");
        expect(text).toContain("gcp_logs with resource runService and name checkout");
    });

    it("a pod gives why it fails and the end of its logs", () => {
        const cluster = { name: "prod", location: "us-central1" } as Parameters<typeof podForAgent>[1];
        const pod = {
            name: "payments-1",
            namespace: "shop",
            phase: "Running",
            reason: "CrashLoopBackOff",
            ready: "0/1",
            restarts: 14,
            node: "n1",
            ip: null,
            containers: [],
            createdAt: null,
        };
        const text = podForAgent("acme-prod", cluster, pod, [{ at: null, text: "panic: db unreachable" }], true);
        expect(text).toContain("Reason: CrashLoopBackOff");
        expect(text).toContain("Logs of the run before its last restart");
        expect(text).toContain("panic: db unreachable");
        expect(text).toContain("namespace shop and pod payments-1");
    });

    it("a VM gives the end of its serial console", () => {
        const vm = { name: "web-1", zone: "us-central1-a", machineType: "e2-medium", status: "RUNNING" } as Parameters<typeof serialForAgent>[1];
        const text = serialForAgent("acme-prod", vm, Array.from({ length: 300 }, (_, i) => `line ${i}`).join("\n"));
        expect(text).toContain("line 299");
        expect(text).not.toContain("line 10\n");
    });
});
