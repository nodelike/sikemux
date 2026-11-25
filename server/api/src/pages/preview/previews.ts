import { existsSync, readFileSync } from "node:fs";

import type { Logger } from "../../log.ts";
import { PictureDrawer } from "./drawer.ts";

export interface Previews {
  drawer: PictureDrawer;
  /** The web app's page with its preview tags, as `server/app/previews.ts` builds it for a share with a title. */
  template: string;
}

/** What draws and tags share links, or null outside a release, which alone holds both pieces. */
export function openPreviews(log: Logger): Previews | null {
  const worker = new URL("./preview-worker.mjs", import.meta.url);
  const page = new URL("../../app/titled-artifact.html", import.meta.url);
  if (!existsSync(worker) || !existsSync(page)) {
    log.warn("not run from a release: share links preview without their title");
    return null;
  }
  return {
    drawer: new PictureDrawer(worker),
    template: readFileSync(page, "utf8"),
  };
}
