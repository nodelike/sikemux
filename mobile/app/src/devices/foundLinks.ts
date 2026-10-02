import type { PairingLink } from '@sikemux/native';

import { linkFromClipboard, openPairing } from './pairing';

/** Links the person scanned or pasted in this app. Pairing from any other link asks first. */
const found = new Set<string>();

function id(link: PairingLink): string {
  return `${link.core}/${link.code}`;
}

export function openFoundLink(link: PairingLink, how: 'push' | 'replace' = 'replace') {
  found.add(id(link));
  openPairing(link, how);
}

export function wasFound(link: PairingLink): boolean {
  return found.has(id(link));
}

export async function pasteFoundLink(how: 'push' | 'replace' = 'push'): Promise<boolean> {
  const link = await linkFromClipboard();
  if (link) openFoundLink(link, how);
  return link !== undefined;
}
