/** The colours a page's dots and accents use, read from the tokens at the top of app.css. */
export type Scene = {
  tint: string;
  alt: string;
  bright: string;
  deep: string;
};

export type SceneName =
  | "you"
  | "blue"
  | "rose"
  | "moss"
  | "amber"
  | "teal"
  | "plan"
  | "sign"
  | `brand-${string}`;

const ORG_SCENES = ["blue", "rose", "moss", "amber", "teal", "plan"] as const;

export function colours(token: string): string[] {
  return getComputedStyle(document.documentElement)
    .getPropertyValue(token)
    .trim()
    .split(/\s+/)
    .filter(Boolean);
}

export function sceneOf(name: SceneName): Scene {
  const named = colours(`--scene-${name}`);
  const [tint = "", alt = "", bright = "", deep = ""] = named.length
    ? named
    : colours("--scene-you");
  return { tint, alt, bright, deep };
}

/** The hash Boring Avatars draws from, so an org's scene and its picture come from one number. */
export function hashOf(id: string): number {
  let hash = 0;
  for (let at = 0; at < id.length; at++)
    hash = ((hash << 5) - hash + id.charCodeAt(at)) | 0;
  return Math.abs(hash);
}

export function orgScene(orgId: string): SceneName {
  return ORG_SCENES[hashOf(orgId) % ORG_SCENES.length] ?? "teal";
}

export function wear(scene: Scene, element = document.documentElement) {
  element.style.setProperty("--tint", scene.tint);
  element.style.setProperty("--alt", scene.alt);
  element.style.setProperty("--bright", scene.bright);
  element.style.setProperty("--deep", scene.deep);
}
