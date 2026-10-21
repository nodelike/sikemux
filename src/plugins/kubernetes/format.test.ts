import { describe, expect, it } from "vitest";
import { age, podTone, shortImage, workloadTone } from "./format";

describe("Kubernetes formatting", () => {
    it("colours a pod by its status as kubectl words it", () => {
        expect(podTone({ status: "Running", ready: 1, containers: 1 })).toBe("live");
        expect(podTone({ status: "Running", ready: 0, containers: 1 })).toBe("warn");
        expect(podTone({ status: "CrashLoopBackOff", ready: 0, containers: 1 })).toBe("danger");
        expect(podTone({ status: "Error (1)", ready: 0, containers: 1 })).toBe("danger");
        expect(podTone({ status: "ImagePullBackOff", ready: 0, containers: 1 })).toBe("danger");
        expect(podTone({ status: "Completed", ready: 0, containers: 1 })).toBe("idle");
        expect(podTone({ status: "Init:0/1", ready: 0, containers: 1 })).toBe("warn");
        expect(workloadTone({ health: "failed" })).toBe("danger");
        expect(workloadTone({ health: "ok" })).toBe("live");
    });

    it("writes ages as kubectl's AGE column does", () => {
        const now = Date.parse("2026-10-19T12:00:00Z");
        expect(age("2026-10-19T11:59:15Z", now)).toBe("45s");
        expect(age("2026-10-19T11:48:00Z", now)).toBe("12m");
        expect(age("2026-10-19T09:00:00Z", now)).toBe("3h");
        expect(age("2026-10-13T12:00:00Z", now)).toBe("6d");
        expect(age(null, now)).toBe("—");
        expect(shortImage("docker.io/library/nginx:alpine")).toBe("nginx:alpine");
    });
});
