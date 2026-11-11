#!/usr/bin/env node
// Writes each target's entitlements with their settings filled in, for signing an
// archive built with signing off: `entitlements.mjs <settings.json> <folder>`.
import fs from 'node:fs';
import path from 'node:path';

const [settingsPath, out] = process.argv.slice(2);
fs.mkdirSync(out, { recursive: true });
for (const { buildSettings: s } of JSON.parse(fs.readFileSync(settingsPath, 'utf8'))) {
  if (!s.CODE_SIGN_ENTITLEMENTS) continue;
  const team = s.DEVELOPMENT_TEAM;
  if (!team) throw new Error(`${s.FULL_PRODUCT_NAME} has no DEVELOPMENT_TEAM in project.yml`);
  const settings = { ...s, AppIdentifierPrefix: `${team}.`, TeamIdentifierPrefix: `${team}.` };
  const text = fs
    .readFileSync(path.resolve(s.SRCROOT, s.CODE_SIGN_ENTITLEMENTS), 'utf8')
    .replace(/\$\((\w+)\)/g, (whole, name) => settings[name] ?? whole);
  if (text.includes('$(')) throw new Error(`${s.CODE_SIGN_ENTITLEMENTS} uses a setting the build does not set`);
  fs.writeFileSync(path.join(out, `${s.FULL_PRODUCT_NAME}.plist`), text);
}
