import { mkdirSync, writeFileSync } from "node:fs";

const folder = `.data/${process.env.SIKEMUX_CHECKOUT ?? "main"}`;
mkdirSync(folder, { recursive: true });
writeFileSync(`${folder}/orders.json`, JSON.stringify([{ id: 1, total: 1999 }]));
console.log(`seeded ${folder}`);
