/*
 * The modules the share page draws a shared chat with, through `@shared/` and `@mac/`. Their own
 * checks cover them; these say only what this page uses, so its check does not walk them.
 */
declare module "@shared/lib/initials" {
  export function initials(
    name: string | null | undefined,
    email: string | null | undefined,
  ): string;
}

declare module "@shared/remote/access" {
  export function platformName(platform: string): string;
}

declare module "@shared/share/link" {
  export interface ShareLink {
    readonly id: string;
    readonly key: string;
  }
  export function parseShareLink(text: string): ShareLink | null;
  export function appShareLink(link: ShareLink, scheme?: string): string;
}

declare module "@shared/share/cipher" {
  import type { ShareLink } from "@shared/share/link";
  export interface ShareCipher {
    open(sealed: string): Promise<string | null>;
    seal(text: string): Promise<string>;
    viewerProof(): Promise<string>;
  }
  export function webCipher(link: ShareLink): ShareCipher;
}

declare module "@shared/share/session" {
  import type { ShareCipher } from "@shared/share/cipher";
  import type { ShareLink } from "@shared/share/link";
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
  import type { ShareSession } from "@shared/share/session";
  export function SharedChat(props: {
    session: ShareSession;
    visible: boolean;
    actions?: ReactNode;
  }): ReactNode;
}

declare module "@mac/themes/bus" {
  export function wearPicked(palette: unknown): void;
}

declare module "@mac/themes/defaultPalette.json" {
  const palette: unknown;
  export default palette;
}

declare module "@mac/styles.css";
