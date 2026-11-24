#!/usr/bin/env node
import { createHash, createPublicKey, verify } from "node:crypto";
import { readFileSync } from "node:fs";

import { appMetadata } from "./app-metadata.mjs";

function fail(message) {
  console.error(`Updater signature verification failed: ${message}`);
  process.exit(1);
}

const [artifactPath, signaturePath, version, keyArgument] =
  process.argv.slice(2);
if (!artifactPath || !signaturePath || !version) {
  fail(
    "usage: verify-updater-signature.mjs <artifact> <signature> <version> [base64 public key]",
  );
}

let publicKeyText;
let signatureText;
try {
  publicKeyText = Buffer.from(
    keyArgument ?? appMetadata()["updater-key"],
    "base64",
  ).toString("utf8");
  signatureText = Buffer.from(
    readFileSync(signaturePath, "utf8").trim(),
    "base64",
  ).toString("utf8");
} catch (error) {
  fail(error instanceof Error ? error.message : String(error));
}

const publicLines = publicKeyText.trim().split("\n");
const signatureLines = signatureText.trim().split("\n");
if (publicLines.length !== 2 || signatureLines.length !== 4)
  fail("invalid minisign envelope");
if (!publicLines[0].startsWith("untrusted comment:"))
  fail("invalid public-key comment");
if (
  !signatureLines[0].startsWith("untrusted comment:") ||
  !signatureLines[2].startsWith("trusted comment: ")
) {
  fail("invalid signature comments");
}

const publicPacket = Buffer.from(publicLines[1], "base64");
const signaturePacket = Buffer.from(signatureLines[1], "base64");
const globalSignature = Buffer.from(signatureLines[3], "base64");
if (
  publicPacket.length !== 42 ||
  signaturePacket.length !== 74 ||
  globalSignature.length !== 64
)
  fail("invalid minisign packet size");
if (
  publicPacket.subarray(0, 2).toString("ascii") !== "Ed" ||
  signaturePacket.subarray(0, 2).toString("ascii") !== "ED"
) {
  fail("unsupported minisign algorithm");
}
if (!publicPacket.subarray(2, 10).equals(signaturePacket.subarray(2, 10)))
  fail("signature key id does not match updater public key");

const spkiPrefix = Buffer.from("302a300506032b6570032100", "hex");
const publicKey = createPublicKey({
  key: Buffer.concat([spkiPrefix, publicPacket.subarray(10)]),
  format: "der",
  type: "spki",
});
const digest = createHash("blake2b512")
  .update(readFileSync(artifactPath))
  .digest();
if (!verify(null, digest, publicKey, signaturePacket.subarray(10)))
  fail("artifact bytes do not match signature");

const trustedComment = Buffer.from(
  signatureLines[2].slice("trusted comment: ".length),
);
const signedFor = trustedComment.toString("utf8").split("\t");
if (!signedFor.includes(`file:${artifactPath.split("/").pop()}`)) {
  fail("trusted comment names a different artifact");
}
if (!signedFor.includes(`version:${version}`)) {
  fail(`trusted comment does not name version ${version}`);
}
if (
  !verify(
    null,
    Buffer.concat([signaturePacket.subarray(10), trustedComment]),
    publicKey,
    globalSignature,
  )
) {
  fail("trusted comment signature is invalid");
}

console.log(`✓ Verified updater signature: ${artifactPath}`);
