import { colours, hashOf, orgScene, sceneOf } from "./scenes.ts";

/* Boring Avatars' marble (MIT, boringdesigners; see BoringAvatars-LICENSE.txt). */
const digit = (number: number, place: number) =>
  Math.floor((number / 10 ** place) % 10);

const unit = (number: number, range: number, place?: number) => {
  const value = number % range;
  return place && digit(number, place) % 2 === 0 ? -value : value;
};

const MARBLE =
  "M32.414 59.35L50.376 70.5H72.5v-71H33.728L26.5 13.381l19.057 27.08L32.414 59.35z";
const MARBLE_OVER =
  "M22.216 24L0 46.75l14.108 38.129L78 86l-3.081-59.276-22.378 4.005 12.972 20.186-23.35 27.395L22.215 24z";

/** `key` keeps the ids inside two drawings of one person on a page apart. */
function marble(seed: string, palette: string[], key: string, radius: number) {
  const number = hashOf(seed);
  const colour = (at: number) => palette[(number + at) % palette.length] ?? "";
  const scale = 1.2 + unit(number * 3, 4) / 10;
  const place = (at: number) => {
    const turned = number * (at + 1);
    return `translate(${unit(turned, 8, 1)} ${unit(turned, 8, 2)}) rotate(${unit(turned, 360, 1)} 40 40) scale(${scale})`;
  };
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 80 80" fill="none" aria-hidden="true"><defs><filter id="f${key}" filterUnits="userSpaceOnUse" x="-40" y="-40" width="160" height="160" color-interpolation-filters="sRGB"><feGaussianBlur stdDeviation="7"/></filter><clipPath id="c${key}"><rect width="80" height="80" rx="${radius}"/></clipPath></defs>` +
    `<g clip-path="url(#c${key})"><rect width="80" height="80" fill="${colour(0)}"/>` +
    `<path filter="url(#f${key})" d="${MARBLE}" fill="${colour(1)}" transform="${place(1)}"/>` +
    `<path filter="url(#f${key})" style="mix-blend-mode:overlay" d="${MARBLE_OVER}" fill="${colour(2)}" transform="${place(2)}"/></g></svg>`
  );
}

export const personSvg = (userId: string, key = "m") =>
  marble(userId, colours("--avatar-people"), key, 40);

export function orgSvg(orgId: string, key = "o") {
  const { tint, alt, deep } = sceneOf(orgScene(orgId));
  return marble(orgId, [tint, alt, deep], key, 18);
}
