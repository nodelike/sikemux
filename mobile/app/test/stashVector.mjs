import { readFileSync } from 'node:fs';

export const shareExtensionSource = (file) => readFileSync(new URL(`../modules/stash/ios/Extension/${file}`, import.meta.url), 'utf8');
