import { readFileSync } from 'node:fs';

export const shareExtensionSource = (file) => readFileSync(new URL(`../../ios/${file}`, import.meta.url), 'utf8');
