import { renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { codeHost, hostApi, registerCodeHost, useHost, usePictureOf } from "./registry";
import { InHost, registerTestHost, TEST_HOST } from "./testHost";

const host = registerTestHost({});

describe("the code host registry", () => {
    it("refuses a host registered twice", () => {
        expect(() => registerCodeHost(host)).toThrow("code host test.host is registered twice");
        expect(codeHost(TEST_HOST)).toBe(host);
    });

    it("has no API for a host nobody registered", () => {
        expect(() => hostApi("nobody.host")).toThrow("no code host nobody.host is registered");
    });

    it("draws nothing that needs a host outside one", () => {
        expect(() => renderHook(() => useHost())).toThrow(/only for what the Git pane draws inside a host/);
    });

    it("prefers the picture sent, and makes one from a name when the host can", () => {
        const named = { ...host, avatarForLogin: (login: string) => `https://example.test/${login}.png` };
        const wrapper = ({ children }: { children: React.ReactNode }) => <InHost host={named}>{children}</InHost>;
        expect(renderHook(() => usePictureOf("ada", "https://sent.png"), { wrapper }).result.current).toBe("https://sent.png");
        expect(renderHook(() => usePictureOf("ada", null), { wrapper }).result.current).toBe("https://example.test/ada.png");
        expect(renderHook(() => usePictureOf(null, null), { wrapper }).result.current).toBeNull();
        expect(renderHook(() => usePictureOf("ada", null)).result.current).toBeNull();
    });
});
