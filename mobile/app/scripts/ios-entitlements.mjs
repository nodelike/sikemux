#!/usr/bin/env node
// Writes each target's entitlements with their settings filled in, for signing an
// archive built with signing off: `ios-entitlements.mjs <settings.json> <folder>`.
import fs from 'node:fs';
import path from 'node:path';

const TEAM = 'D577WD6Z5U';
const [settingsPath, out] = process.argv.slice(2);
fs.mkdirSync(out, { recursive: true });
for (const { buildSettings: s } of JSON.parse(fs.readFileSync(settingsPath, 'utf8'))) {
  if (!s.CODE_SIGN_ENTITLEMENTS) continue;
  const text = fs
    .readFileSync(path.resolve(s.SRCROOT, s.CODE_SIGN_ENTITLEMENTS), 'utf8')
    .replaceAll('$(AppIdentifierPrefix)', `${TEAM}.`)
    .replaceAll('$(TeamIdentifierPrefix)', `${TEAM}.`);
  if (text.includes('$(')) throw new Error(`${s.CODE_SIGN_ENTITLEMENTS} uses a setting this step does not fill in`);
  fs.writeFileSync(path.join(out, `${s.FULL_PRODUCT_NAME}.plist`), text);
}
