const { body, forms } = document;
const { poll } = body.dataset;
forms[0]?.submit();
if (poll)
  setInterval(async () => {
    try {
      const answer = await fetch(poll, { cache: "no-store" });
      if (answer.ok && (await answer.text()) !== "waiting") location.reload();
    } catch {}
  }, 1000);

const canvas = document.querySelector("canvas");
const mark = document.querySelector(".mark");
const icon = mark.querySelector("svg");
const { tint } = canvas.dataset;
const tone = body.className;
const dots = drawDots(canvas, {
  mark,
  quiet: () => [...document.querySelectorAll("main > :not(.mark), footer")],
});
dots?.look({ tint, tone });
dots?.show({ svg: icon.outerHTML, color: getComputedStyle(icon).color });
if (tone === "worked") setTimeout(() => dots?.ring(tint), 350);
