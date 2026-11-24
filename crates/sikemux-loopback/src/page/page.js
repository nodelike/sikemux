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
drawDots(canvas, document.querySelector(".mark"), {
  tint: canvas.dataset.tint,
  failed: body.classList.contains("failed"),
  waiting: body.classList.contains("waiting"),
});
