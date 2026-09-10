import { describe, expect, it } from "vitest";
import { mountShaderField, shaderFieldCount, unmountShaderField } from "./shaderField";

/*
 * jsdom has no WebGL, which is the same situation as a machine whose driver is
 * blocklisted. The contract under test is that this is silent: a field is
 * decoration, so a caller must never have to know whether one arrived.
 */
describe("shaderField", () => {
    it("claims no surface when WebGL is unavailable", () => {
        const host = document.createElement("span");
        document.body.append(host);

        mountShaderField(host, "empty");

        expect(shaderFieldCount()).toBe(0);
        expect(host.dataset.shaderField).toBeUndefined();
        host.remove();
    });

    it("unmounts a host that never got a surface without throwing", () => {
        const host = document.createElement("span");
        expect(() => unmountShaderField(host)).not.toThrow();
        expect(shaderFieldCount()).toBe(0);
    });

    it("leaves no canvas behind for the interface to work around", () => {
        const host = document.createElement("span");
        document.body.append(host);

        mountShaderField(host, "onboarding");
        unmountShaderField(host);

        expect(host.querySelector("canvas")).toBeNull();
        host.remove();
    });
});
