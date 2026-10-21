import { describe, expect, it } from "vitest";
import { podBrief } from "./brief";
import { WORKER_DETAIL } from "./testing";

describe("a pod brief", () => {
    it("tells an agent what is down, why, and where to look next", () => {
        const brief = podBrief(WORKER_DETAIL, { context: "kind-dev", namespace: "shop" });
        expect(brief).toContain("Pod worker-546f6f965-9swqn (cluster kind-dev, namespace shop) is CrashLoopBackOff, 0/1 ready, 6 restarts.");
        expect(brief).toContain("container worker (docker.io/library/busybox:1.36): waiting, CrashLoopBackOff");
        expect(brief).toContain("last run ended: Error (exit 1)");
        expect(brief).toContain("- BackOff ×14: Back-off restarting failed container worker");
        expect(brief).toContain("k8s_logs with previous");
        expect(brief).not.toContain("QUEUE_URL");
    });
});
