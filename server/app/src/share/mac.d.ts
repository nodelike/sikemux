/*
 * The Mac app's modules the share page draws a shared chat with, through `@mac/`. The Mac's own
 * check covers them; these say only what this page uses, so its check does not walk the Mac app.
 */
declare module "@mac/lib/initials" {
  export function initials(
    name: string | null | undefined,
    email: string | null | undefined,
  ): string;
}

declare module "@mac/remote/access" {
  export function platformName(platform: string): string;
}

declare module "@mac/share/link" {
  export interface ShareLink {
    readonly id: string;
    readonly key: string;
  }
  export function parseShareLink(text: string): ShareLink | null;
  export function appShareLink(link: ShareLink, scheme?: string): string;
}

declare module "@mac/share/cipher" {
  import type { ShareLink } from "@mac/share/link";
  export interface ShareCipher {
    open(sealed: string): Promise<string | null>;
    seal(text: string): Promise<string>;
    viewerProof(): Promise<string>;
  }
  export function webCipher(link: ShareLink): ShareCipher;
}

declare module "@mac/share/session" {
  import type { ShareCipher } from "@mac/share/cipher";
  import type { ShareLink } from "@mac/share/link";
  export class ShareSession {
    constructor(options: {
      link: ShareLink;
      url: string;
      cipher: ShareCipher;
      token(fresh: boolean): Promise<string | null>;
    });
    start(): void;
    stop(): void;
  }
}

declare module "@mac/share/SharedChat" {
  import type { ReactNode } from "react";
  import type { ShareSession } from "@mac/share/session";
  export function SharedChat(props: {
    session: ShareSession;
    visible: boolean;
    actions?: ReactNode;
  }): ReactNode;
}

declare module "@mac/themes/bus" {
  export function applyTheme(id: string): void;
}

declare module "@mac/themes" {
  export const DEFAULT_THEME_ID: string;
}

declare module "@mac/styles.css";
