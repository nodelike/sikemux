import {
  MAX_STASH_FILE_BYTES,
  MAX_STASH_TEXT,
  STASH_HEADERS,
  stashUploadHeaders,
} from "@sikemux/protocol/stash";
import { describe, expect, it } from "vitest";

import deviceRequest from "../../protocol/vectors/device-request.json" with { type: "json" };
import hostRequest from "../../protocol/vectors/host-request.json" with { type: "json" };
import live from "../../protocol/vectors/live.json" with { type: "json" };
import stashVector from "../../protocol/vectors/stash.json" with { type: "json" };
import vector from "../../protocol/vectors/registration.json" with { type: "json" };
import {
  bodySha256,
  liveMessage,
  registrationMessage,
  requestMessage,
  signedBy,
} from "../src/devices/signature.ts";

describe("registration signatures", () => {
  it("verify the vector the core signs", () => {
    const message = registrationMessage(
      vector.nonce,
      vector.userId,
      vector.key,
    );
    expect(message).toBe(vector.message);
    expect(signedBy(vector.key, message, vector.signature)).toBe(true);
  });

  it("fail for any other account, challenge or key", () => {
    expect(
      signedBy(
        vector.key,
        registrationMessage(vector.nonce, "user_other", vector.key),
        vector.signature,
      ),
    ).toBe(false);
    expect(
      signedBy(
        vector.key,
        registrationMessage("0".repeat(64), vector.userId, vector.key),
        vector.signature,
      ),
    ).toBe(false);
    expect(signedBy("0".repeat(64), vector.message, vector.signature)).toBe(
      false,
    );
  });
});

describe("live connection signatures", () => {
  it("verify the vector the core and the phone sign", () => {
    const message = liveMessage(live.nonce, live.key);
    expect(message).toBe(live.message);
    expect(signedBy(live.key, message, live.signature)).toBe(true);
  });

  it("never stand in for a registration, nor a registration for them", () => {
    expect(live.key).toBe(vector.key);
    expect(live.nonce).toBe(vector.nonce);
    expect(signedBy(live.key, vector.message, live.signature)).toBe(false);
    expect(
      signedBy(vector.key, liveMessage(live.nonce, live.key), vector.signature),
    ).toBe(false);
  });

  it("fail for another challenge or key", () => {
    expect(
      signedBy(live.key, liveMessage("0".repeat(64), live.key), live.signature),
    ).toBe(false);
    expect(
      signedBy(
        "0".repeat(64),
        liveMessage(live.nonce, "0".repeat(64)),
        live.signature,
      ),
    ).toBe(false);
  });
});

describe("host request signatures", () => {
  it("verify the vector the core signs", () => {
    const message = requestMessage(
      "host",
      hostRequest.method,
      hostRequest.path,
      String(hostRequest.time),
      bodySha256(new TextEncoder().encode(hostRequest.body)),
    );
    expect(message).toBe(hostRequest.message);
    expect(signedBy(hostRequest.key, message, hostRequest.signature)).toBe(
      true,
    );
  });
});

describe("a phone's stash request signatures", () => {
  it("verify the vector the phone signs", () => {
    const message = requestMessage(
      "device",
      deviceRequest.method,
      deviceRequest.path,
      String(deviceRequest.time),
      bodySha256(new TextEncoder().encode(deviceRequest.body)),
    );
    expect(message).toBe(deviceRequest.message);
    expect(signedBy(deviceRequest.key, message, deviceRequest.signature)).toBe(
      true,
    );
  });
});

describe("the stash upload's contract", () => {
  it("is what the vector every sender reads says", () => {
    expect(MAX_STASH_FILE_BYTES).toBe(stashVector.maxFileBytes);
    expect(MAX_STASH_TEXT).toBe(stashVector.maxText);
    expect(STASH_HEADERS).toEqual(stashVector.headers);
    const headers = stashUploadHeaders({
      name: "é",
      note: "é",
      target: "é",
      prompt: "é",
    });
    const encoded = Object.entries(STASH_HEADERS)
      .filter(([, header]) => headers[header] !== "é")
      .map(([field]) => field);
    expect(encoded).toEqual(stashVector.encoded);
  });
});
