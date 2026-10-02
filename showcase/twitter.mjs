import { execFileSync } from "node:child_process";
import { resolve } from "node:path";
import { createServer } from "vite";

const server = await createServer({
  configFile: resolve(import.meta.dirname, "vite.config.ts"),
  logLevel: "warn",
});
await server.listen();

const url = `http://localhost:${server.config.server.port}/showcase/twitter.html`;
execFileSync("open", [url]);
console.log(`${url}\nPick a view and press Download; it saves to ~/Downloads.`);
