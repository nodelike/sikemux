import { describe, expect, it } from "vitest";
import { actionsFor, ago, appTone, eventTone, powerTone, region, size } from "./format";
import { stateTone } from "./components/Resources";
import { warningFor } from "./components/Machines";
import { sshCommand } from "./components/MachineView";

describe("azure words", () => {
    it("colours a machine, app and event by how it is doing", () => {
        expect(powerTone("running")).toBe("live");
        expect(powerTone("deallocated")).toBe("idle");
        expect(powerTone("starting")).toBe("warn");
        expect(appTone("Running")).toBe("live");
        expect(appTone("Stopped")).toBe("idle");
        expect(eventTone({ status: "Failed", level: "Error" })).toBe("danger");
        expect(eventTone({ status: "Succeeded", level: "Informational" })).toBe("live");
        expect(stateTone("Succeeded")).toBe("live");
        expect(stateTone("Failed")).toBe("danger");
        expect(stateTone(null)).toBe("idle");
    });

    it("writes regions, sizes and times as people say them", () => {
        expect(region("uaenorth")).toBe("UAE North");
        expect(region("centralindia")).toBe("Central India");
        expect(region("westus2")).toBe("West US 2");
        expect(region("")).toBe("—");
        expect(size("Standard_D2s_v5")).toBe("D2s_v5");
        const now = Date.parse("2026-10-19T12:00:00Z");
        expect(ago("2026-10-19T11:00:00Z", now)).toBe("1h");
        expect(ago(null, now)).toBe("—");
    });

    it("offers only what a machine's power state allows, and warns before stopping", () => {
        expect(actionsFor({ power: "running" }).map((each) => each.action)).toEqual(["restart", "powerOff", "deallocate"]);
        expect(actionsFor({ power: "deallocated" }).map((each) => each.action)).toEqual(["start"]);
        expect(warningFor("deallocate", "vm")?.body).toMatch(/public IP/);
        expect(warningFor("powerOff", "vm")?.body).toMatch(/keeps costing/);
        expect(warningFor("start", "vm")).toBeNull();
    });

    it("offers ssh only for a Linux machine that can be reached", () => {
        expect(sshCommand("20.74.12.8", "Linux")).toEqual(["ssh", "20.74.12.8"]);
        expect(sshCommand("20.74.12.8", "Windows")).toBeNull();
        expect(sshCommand("", "Linux")).toBeNull();
    });
});
