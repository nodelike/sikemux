import { describe, expect, it } from "vitest";
import { initials } from "./parts";
import { healthTone } from "./Releases";
import { tokensPage } from "./SentrySignIn";

describe("sentry pane words", () => {
    it("links to where a personal token is made, on sentry.io or a self-hosted Sentry", () => {
        expect(tokensPage("")).toBe("https://sentry.io/settings/account/api/auth-tokens/");
        expect(tokensPage("sentry.acme.dev/")).toBe("https://sentry.acme.dev/settings/account/api/auth-tokens/");
    });

    it("colours crash-free rates as Sentry does and writes initials", () => {
        expect(healthTone(99.9)).toBe("live");
        expect(healthTone(98.5)).toBe("warn");
        expect(healthTone(96)).toBe("danger");
        expect(healthTone(null)).toBe("idle");
        expect(initials("Edon Ramos")).toBe("ER");
        expect(initials("maya@acme.dev")).toBe("MA");
    });
});
