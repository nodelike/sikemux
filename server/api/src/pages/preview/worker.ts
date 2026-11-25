import { setPriority } from "node:os";
import { parentPort } from "node:worker_threads";

import { drawCard, type Card } from "./card.ts";

// On Linux this lowers this thread alone, so a drawing gives way to the requests being served.
setPriority(0, 10);

parentPort?.on("message", (card: Card) => {
  void drawCard(card).then((png) => parentPort?.postMessage(png));
});
