import type { BackdropLook, LookReply } from "@shared/wire.generated";
import { expect, test } from "vitest";

import { pickLook } from "../src/look/pickLook.ts";

const look: BackdropLook = {
  strength: 1,
  inks: [null, "#a277ff", "#e7e5ef"],
  ownColours: false,
  pattern: "bayer8",
  dotSize: 2,
  contrast: 0,
  brightness: 0,
  glow: 0.4,
  glowSize: 0.4,
  coverage: "band",
};
const reply = (
  picture: string | null,
  personAtWindow: number | null,
): LookReply => ({
  result: "look",
  texture: true,
  look,
  picture,
  personAtWindow,
});

// 18 Nov: an API deploy reconnected every host within a second, so the newest lastSeenAt was a
// host the person was not at, and its look, with no picture, left the page on the grain.
test("the host whose window the person was at wins over the one seen last", () => {
  const throne = reply(
    "data:image/jpeg;base64,throne",
    Date.parse("2026-11-17T19:41:00Z"),
  );
  const picked = pickLook([
    { seenAt: "2026-11-17T19:40:48.720Z", reply: reply(null, null) },
    { seenAt: "2026-11-17T19:40:47.961Z", reply: reply(null, null) },
    { seenAt: "2026-11-17T19:40:47.318Z", reply: throne },
    {
      seenAt: "2026-11-17T19:40:46.937Z",
      reply: { result: "refused", reason: "unknown" },
    },
  ]);
  expect(picked?.picture).toBe("data:image/jpeg;base64,throne");
});
