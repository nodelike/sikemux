import type { useAuth } from "@clerk/react";
import type { Org } from "@sikemux/protocol";
import { useSyncExternalStore } from "react";

import { bytesApi, signedInToken } from "./api.ts";
import { orgSvg } from "./avatarSvg.ts";
import type { Picture } from "./dots.ts";

type GetToken = ReturnType<typeof useAuth>["getToken"];

type Logo =
  | { state: "loading" }
  | { state: "failed" }
  | { state: "ready"; picture: Blob; url: string };

type HasLogo = Pick<Org, "id" | "logo">;

const BLANK = { svg: '<svg xmlns="http://www.w3.org/2000/svg"/>' };
const SIDE = 512;
const DARK = 70;

export const LOGO_TYPES = ["image/png", "image/jpeg", "image/webp"];

/** By `Org.logo`, which changes with every new picture. */
const logos = new Map<string, Logo>();
const watchers = new Set<() => void>();

function put(id: string, logo: Logo) {
  logos.set(id, logo);
  for (const changed of watchers) changed();
}

function watch(changed: () => void) {
  watchers.add(changed);
  return () => watchers.delete(changed);
}

const ready = (picture: Blob): Logo => ({
  state: "ready",
  picture,
  url: URL.createObjectURL(picture),
});

/** Fetches the logos this tab doesn't hold yet. */
export function loadLogos(orgs: Org[], getToken: GetToken) {
  for (const { id, logo } of orgs) {
    if (!logo || logos.has(logo)) continue;
    put(logo, { state: "loading" });
    void signedInToken(getToken, "see the org's logo")
      .then((token) => bytesApi(token, `/v1/orgs/${id}/logo`))
      .then((response) => response.blob())
      .then(
        (picture) => put(logo, ready(picture)),
        () => put(logo, { state: "failed" }),
      );
  }
}

export const logoFailed = (id: string) => put(id, { state: "failed" });

export function useLogo(org: HasLogo | undefined): Logo | undefined {
  return useSyncExternalStore(watch, () =>
    org?.logo ? logos.get(org.logo) : undefined,
  );
}

/** What the dots draw for an org: its logo, or its marble when it has none. */
export function useOrgMark(org: HasLogo | undefined): Picture {
  const logo = useLogo(org);
  if (!org || logo?.state === "loading") return BLANK;
  return logo?.state === "ready"
    ? { image: logo.url }
    : { svg: orgSvg(org.id, "mark") };
}

/** Sends an org its new logo, and answers the org with it. */
export async function sendLogo(
  orgId: string,
  picture: Blob,
  getToken: GetToken,
): Promise<Org> {
  const token = await signedInToken(getToken, "change the org's logo");
  const response = await bytesApi(token, `/v1/orgs/${orgId}/logo`, {
    method: "PUT",
    body: picture,
  });
  const org = (await response.json()) as Org;
  if (org.logo) put(org.logo, ready(picture));
  return org;
}

/**
 * A chosen picture as the logo to send: whole, in the middle of a square. One drawn dark on nothing
 * gets a white square behind it, since every page is near black.
 */
export async function fitLogo(file: File): Promise<Blob> {
  const picture = await createImageBitmap(file).catch(() => undefined);
  if (!picture)
    throw new Error("It couldn't be read. Choose another PNG, JPG or WebP.");
  const longest = Math.max(picture.width, picture.height);
  const side = Math.min(SIDE, longest);
  const sheet = document.createElement("canvas");
  sheet.width = sheet.height = side;
  const ink = sheet.getContext("2d");
  if (!ink) throw new Error("This browser can't draw it. Try another browser.");
  const [width, height] = [picture.width, picture.height].map(
    (length) => (length * side) / longest,
  ) as [number, number];
  ink.drawImage(
    picture,
    (side - width) / 2,
    (side - height) / 2,
    width,
    height,
  );
  picture.close();
  if (darkOnNothing(ink.getImageData(0, 0, side, side).data)) {
    ink.globalCompositeOperation = "destination-over";
    ink.fillStyle = "#fff";
    ink.fillRect(0, 0, side, side);
  }
  const drawn = await new Promise<Blob | null>((done) =>
    sheet.toBlob(done, "image/webp", 0.92),
  );
  if (!drawn) throw new Error("This browser can't draw it. Try another one.");
  return drawn;
}

function darkOnNothing(pixels: Uint8ClampedArray): boolean {
  let [light, covered] = [0, 0];
  for (let at = 0; at < pixels.length; at += 4) {
    const [red = 0, green = 0, blue = 0, alpha = 0] = pixels.subarray(
      at,
      at + 4,
    );
    light += (0.2126 * red + 0.7152 * green + 0.0722 * blue) * alpha;
    covered += alpha;
  }
  const whole = (pixels.length / 4) * 255;
  return covered > 0 && covered < whole * 0.9 && light / covered < DARK;
}
