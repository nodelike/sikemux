import type { ReactNode } from "react";

/* No plugin runs on the page, so tool rows and links wear no plugin's mark. */
interface PagePlugin {
  readonly id: string;
  readonly mark?: (size: number) => ReactNode;
}

export function frontendPlugin(_id: string): PagePlugin | undefined {
  return undefined;
}

export function linkPlugin(_href: string): PagePlugin | undefined {
  return undefined;
}

export function isPluginEnabled(
  _id: string,
  _disabled: readonly string[],
): boolean {
  return false;
}
