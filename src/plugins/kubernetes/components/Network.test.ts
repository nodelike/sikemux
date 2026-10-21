import { describe, expect, it } from "vitest";
import { localPortFor, routeUrl, servicePort } from "./Network";
import { size } from "./Config";

describe("service ports", () => {
    it("reads the port a service answers on and picks a local one to forward to", () => {
        expect(servicePort("80→8080/TCP (node 30080)")).toBe(80);
        expect(servicePort("5432/TCP")).toBe(5432);
        expect(servicePort("http")).toBeNull();
        expect(localPortFor(80)).toBe(8080);
        expect(localPortFor(5432)).toBe(5432);
    });

    it("links an ingress route only when its host is a real one", () => {
        expect(routeUrl({ host: "shop.acme.dev", path: "/api" }, ["shop.acme.dev"])).toBe("https://shop.acme.dev/api");
        expect(routeUrl({ host: "shop.local", path: "/" }, [])).toBe("http://shop.local/");
        expect(routeUrl({ host: "*", path: "/" }, [])).toBeNull();
        expect(routeUrl({ host: "*.acme.dev", path: "/" }, [])).toBeNull();
    });

    it("writes key sizes in units people read", () => {
        expect(size(21)).toBe("21 B");
        expect(size(2048)).toBe("2.0 KB");
        expect(size(3 * 1024 * 1024)).toBe("3.0 MB");
    });
});
