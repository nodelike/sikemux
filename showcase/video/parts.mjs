/*
 * Beats more than one video plays, so a post and the launch reel are cuts of
 * the same moments. Each takes the director and leaves the camera where it
 * says, for whatever plays next.
 */

import { STAGE } from "../stage.mjs";

export const OTTER = "tab-rail";
export const MAPLE = "agent-replay";
export const WREN = "agent-hero";
export const FLINT = "agent-palette";

export const composer = '[role="textbox"][aria-label="Message agent"]';
const scope = (name) => ({
  text: name,
  selector: ".swipe-page.is-current .scope-track button",
  exact: false,
});
const card = (title) => ({
  selector: ".kanban-card",
  text: title,
  exact: false,
});

/** Opens each chat a video visits, so its agent has started, then leaves every chat idle. */
export async function startChats(s, windows) {
  for (const [session, window] of windows) {
    await s.open(session, window);
    await s.settle(700);
  }
  await s.app(() => showcase.backend.settleLive());
}

/**
 * The person asks Otter to pass work on to Maple, tagging her with @. The menu
 * filters off camera, so it shows once, settled, with Maple picked, and stays a beat.
 */
export async function askOtter(s, { perSecond = 15, menuHold = 1000 } = {}) {
  await s.click(composer, { at: { x: 0.9, y: 0.75 } });
  await s.type("When ", { perSecond });
  await s.offCamera(async () => {
    await s.page.keyboard.type("@Ma");
    await s.wait(500);
  });
  await s.wait(menuHold);
  await s.key("Enter", { after: 120 });
  await s.type("lands the ledger import, take the refunds bug.", { perSecond });
  await s.wait(250);
  await s.key("Enter");
}

/** Right after the send, Otter starts and messages Maple, framed on the call. */
export async function otterMessagesMaple(s, { hold = 1700 } = {}) {
  await s.live({ agent: OTTER, steps: 2, pace: 0.6 });
  await s.wait(300);
  s.live({ agent: OTTER, steps: 1, wait: false });
  await s.appear(lastTools);
  await s.zoom(lastTools, { scale: 1.8, at: { x: 0.42, y: 0.5 } });
  await s.wait(hold);
}

/** One swipe over to Maple's chat: Otter's message arrives, and she answers once her tests pass. */
export async function mapleAnswers(
  s,
  { letterHold = 1700, replyHold = 1600 } = {},
) {
  await s.zoomOut({ wait: false });
  await s.app(() => showcase.backend.cue("maple-answers"));
  await swipeTabs(s);
  await s.live({ agent: MAPLE, steps: 1, pace: 0.3 });
  s.mark("message");
  await s.live({ agent: MAPLE, steps: 1, pace: 0.3 });
  await s.zoom(
    { selector: ".agent-letter", last: true },
    { scale: 1.7, at: { x: 0.5, y: 1.3 } },
  );
  await s.wait(letterHold);
  await s.live({ agent: MAPLE, steps: 4, pace: 0.15, shortest: 100 });
  await s.wait(replyHold);
}

/** On the project's board, Maple's card moves to Done, and Otter takes the refunds card it was waiting on. */
export async function boardMoves(s, { hold = 1000 } = {}) {
  await s.zoomOut({ wait: false });
  await s.click({ selector: ".side-rail *", text: "Agent board" });
  const otterLane = ".kanban-columns";
  await s.steady(otterLane, { frames: 6 });
  s.zoom(otterLane, { pad: 40, wait: false });
  await s.move(card("Ledger import onto amounts"));
  await s.still();
  s.mark("card");
  await s.live({ agent: MAPLE, steps: 1, pace: 0, shortest: 0 });
  await s.wait(700 + hold);
  await s.app(() => showcase.backend.cue("otter-takes-refunds"));
  await s.live({ agent: OTTER, steps: 2, pace: 0.15, shortest: 100 });
  s.mark("card");
  await s.live({ agent: OTTER, steps: 2, pace: 0.15, shortest: 100 });
  await s.move(card("Totals are off by a cent"));
  await s.live({ agent: OTTER, steps: 1, pace: 0, shortest: 0 });
  await s.wait(hold);
  s.live({ agent: OTTER, wait: false });
  s.live({ agent: MAPLE, wait: false });
}

const railPage = ".tab-rail .swipe-page.is-current";

/** Swipes the tab rail over to its next page and back, each in one unhurried motion. */
export async function swipeRail(s, { hold = 1000, duration = 420 } = {}) {
  await s.swipe(railPage, { at: { x: 0.6, y: 0.2 }, duration });
  await s.wait(hold);
  await s.swipe(railPage, { pixels: -160, at: { x: 0.3, y: 0.2 }, duration });
}

/** Swipes the middle of the window across to the session's next tab, or back with a negative `pixels`. */
export const swipeTabs = (s, { pixels = 600, duration = 360 } = {}) =>
  s.swipe(".window-area", { pixels, duration });

export const flipScope = (s, name, options) => s.click(scope(name), options);

/** Picks a project in the side rail, which swaps the whole workspace to it. */
export const openProject = (s, name, options) =>
  s.click({ selector: ".side-rail .proj-row *", text: name }, options);

/** Wren's desk open beside its chat on the moodboard app's cover, where it left off, before it is asked to look again. */
export async function wrenAtItsDesk(s) {
  await s.app(async () => {
    showcase.cmd.openDesk("agent-hero", { focus: false });
    const tabs = await import("/showcase/browserPage.ts");
    tabs.showBrowserPage("agent-hero", {
      url: "http://localhost:5173/",
      title: "Moodboard Studio · Issue 07",
      picture: "/showcase/pages/moodboard-cover.png",
    });
  });
}

const lastTools = { selector: ".chat-tools", last: true };

/**
 * Wren opens the app in its tab and clicks through to the shop, framed on its
 * tool calls beside the tab, each held long enough to read; the screenshot lands in the chat.
 */
export async function browserCheck(s, { hold = 1300 } = {}) {
  const chat = await s.rect(".agent-chat-pane");
  // Wren's chat in the middle, the side rail's edge on the left, the tab only a sliver on the right.
  const width = chat.width * 1.47;
  const height = (width * STAGE.height) / STAGE.width;
  const around = async (target) => {
    const box = await s.rect(target);
    return {
      x: chat.x - chat.width * 0.29,
      y: box.y + box.height / 2 - height / 2,
      width,
      height,
    };
  };
  s.move(".agent-chat-pane", { at: { x: 0.78, y: 0.8 }, wait: false });
  await s.live({ agent: WREN, steps: 2, pace: 0.4 });
  await s.zoom(await around({ selector: ".chat-message", last: true }), {
    pad: 0,
  });
  await s.live({ agent: WREN, steps: 1, shortest: 0, pace: 0 });
  await s.wait(hold);
  await s.live({ agent: WREN, steps: 1, shortest: 0, pace: 0 });
  await s.wait(hold);
  // The chat scrolls the reply and its picture in at once, so the camera stands back while they arrive.
  await s.zoomOut({ wait: false });
  const reply = s.live({ agent: WREN, pace: 0.25, shortest: 120 });
  await s.appear({ selector: ".chat-image", last: true });
  s.mark("shot");
  await reply;
  await s.steady({ selector: ".chat-image", last: true }, { frames: 6 });
  await s.zoom(await around({ selector: ".chat-image", last: true }), {
    pad: 0,
  });
  await s.wait(1000);
}

/** Swipes from Wren's tab to Flint's, and Flint answers with a chart drawn in the chat, framed whole. */
export async function chartAnswer(s, { hold = 4500 } = {}) {
  await s.zoomOut({ wait: false });
  await s.app(() => showcase.backend.cue("flint-charts-the-palettes"));
  s.live({ agent: FLINT, pace: 0.2, shortest: 100, wait: false });
  await swipeTabs(s);
  const chart = { selector: ".chat-page", last: true };
  await s.steady(chart, { frames: 6 });
  await s.zoom([{ selector: ".chat-message", last: true }, chart], { pad: 70 });
  await s.wait(hold);
}

const ALLOW = { stage: "img.notch" };

/**
 * Otter asks to run a command, the island drops from the notch to ask, and the
 * person allows it there, which answers the chat's card too; the island goes
 * straight on to Finished.
 */
export async function approveFromNotch(
  s,
  { ask = 1400, hold = 1100, showCard = 0 } = {},
) {
  await s.app(() => showcase.backend.cue("otter-runs-the-rail-tests"));
  await s.live({ agent: OTTER, steps: 3, pace: 0.35, shortest: 150 });
  if (showCard) {
    await s.zoom(".chat-permission", { scale: 1.7 });
    await s.wait(showCard);
  }
  await s.zoom(
    { stage: "img.notch" },
    { scale: 2.6, at: { x: 0.5, y: 0.25 }, wait: false },
  );
  await s.island("P1-permission");
  await s.still();
  await s.wait(ask);
  s.cursor.shape = "hand";
  await s.click(ALLOW, { at: { x: 0.685, y: 0.56 }, pause: 160, after: 0 });
  s.mark("allow");
  s.cursor.shape = null;
  await s.app(() =>
    document.querySelector(".chat-permission-actions button")?.click(),
  );
  s.live({ agent: OTTER, steps: 2, pace: 0.15, shortest: 100, wait: false });
  await s.island("C2-working");
  await s.wait(300);
  await s.island("P2-finished");
  await s.wait(hold);
  await s.island("C4b-working-and-done");
  await s.live({ agent: OTTER, wait: false });
}
