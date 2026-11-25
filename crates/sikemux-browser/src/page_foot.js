((covered) => {
  const held = "__sikemuxFoot";
  let sheet = window[held];
  if (!sheet) {
    sheet = new CSSStyleSheet();
    Object.defineProperty(window, held, { value: sheet });
  }
  if (!document.adoptedStyleSheets.includes(sheet)) {
    document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
  }
  sheet.replaceSync("");
  const own = parseFloat(getComputedStyle(document.body).paddingBottom) || 0;
  sheet.replaceSync(`body { padding-bottom: ${own + covered}px !important }`);
})
