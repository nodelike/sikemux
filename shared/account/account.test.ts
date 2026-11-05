import { describe, expect, it } from "vitest";

import { chooseFactor } from "./account";

const EMAIL = { strategy: "email_code", to: "c***@nodelike.com", emailAddressId: "idn_1" } as const;

describe("chooseFactor", () => {
    it("takes the password when the account has one", () => {
        expect(chooseFactor([{ strategy: "email_code", emailAddressId: "idn_1" }, { strategy: "password" }])).toEqual({ strategy: "password" });
    });

    it("sends a code to the email for an account signed in with Google or GitHub", () => {
        expect(chooseFactor([{ strategy: "email_code", emailAddressId: "idn_1", safeIdentifier: "c***@nodelike.com" }])).toEqual(EMAIL);
    });

    it("has nothing to offer without either", () => {
        expect(chooseFactor([{ strategy: "passkey" }])).toBeUndefined();
    });
});
