import { describe, expect, it } from "vitest";
import { isRunning, percent, portLabel, portUrl, shortImage, toneOf } from "./format";

describe("Docker's formatting", () => {
    it("names a published port as a browser opens it, and an unpublished one by its number", () => {
        expect(portLabel({ private: 80, public: 8080, ip: "0.0.0.0", protocol: "tcp" })).toBe("localhost:8080");
        expect(portLabel({ private: 53, public: 5353, ip: "127.0.0.1", protocol: "udp" })).toBe("127.0.0.1:5353/udp");
        expect(portLabel({ private: 9229, public: null, ip: null, protocol: "tcp" })).toBe("9229/tcp");
        expect(portUrl({ private: 80, public: 8080, ip: null, protocol: "tcp" })).toBe("http://localhost:8080");
        expect(portUrl({ private: 53, public: 5353, ip: null, protocol: "udp" })).toBeNull();
    });

    it("colours a container by its state, warning when it is unhealthy and alarming when it failed", () => {
        expect(toneOf({ state: "running", status: "Up 2 hours (healthy)" })).toBe("live");
        expect(toneOf({ state: "running", status: "Up 2 hours (unhealthy)" })).toBe("warn");
        expect(toneOf({ state: "restarting", status: "Restarting (1) 3 seconds ago" })).toBe("warn");
        expect(toneOf({ state: "exited", status: "Exited (137) 5 minutes ago" })).toBe("danger");
        expect(toneOf({ state: "exited", status: "Exited (0) 5 minutes ago" })).toBe("idle");
        expect(toneOf({ state: "created", status: "Created" })).toBe("idle");
    });

    it("reads images as people say them and keeps percentages in range", () => {
        expect(shortImage("docker.io/library/postgres:16")).toBe("postgres:16");
        expect(shortImage("ghcr.io/acme/web:1.2")).toBe("ghcr.io/acme/web:1.2");
        expect(isRunning({ state: "restarting" })).toBe(true);
        expect(percent(50, 200)).toBe(25);
        expect(percent(300, 200)).toBe(100);
        expect(percent(1, 0)).toBe(0);
    });
});
