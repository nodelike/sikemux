const highlight = {
  keyword: "#a277ff",
  string: "#61ffca",
  comment: "#6d6a7c",
  number: "#ffca85",
  function: "#ff6ac1",
  type: "#ffca85",
  variable: "#e7e5ef",
  property: "#82d9ff",
  tag: "#ff6ac1",
  operator: "#8b8898",
  link: "#61ffca",
  invalid: "#ff6767",
  meta: "#8b8898",
  heading: "#a277ff",
};

export const auraNoir = {
  name: "aura-noir",
  type: "dark",
  colors: {
    "editor.background": "#0f0f13",
    "editor.foreground": "#e7e5ef",
  },
  tokenColors: [
    { scope: ["comment", "punctuation.definition.comment"], settings: { foreground: highlight.comment, fontStyle: "italic" } },
    { scope: ["string", "string.quoted", "string.template", "string.regexp"], settings: { foreground: highlight.string } },
    { scope: ["constant.numeric", "constant.language", "constant.character"], settings: { foreground: highlight.number } },
    { scope: ["keyword", "storage", "storage.type", "storage.modifier"], settings: { foreground: highlight.keyword } },
    { scope: ["entity.name.function", "support.function", "meta.function-call"], settings: { foreground: highlight.function } },
    { scope: ["entity.name.type", "entity.name.class", "support.type", "support.class"], settings: { foreground: highlight.type } },
    { scope: ["variable", "variable.other.readwrite", "variable.parameter"], settings: { foreground: highlight.variable } },
    {
      scope: ["variable.other.property", "support.variable.property", "meta.object-literal.key", "support.type.property-name"],
      settings: { foreground: highlight.property },
    },
    { scope: ["entity.name.tag", "punctuation.definition.tag"], settings: { foreground: highlight.tag } },
    { scope: ["keyword.operator", "punctuation", "meta.brace"], settings: { foreground: highlight.operator } },
    { scope: ["markup.heading", "entity.name.section"], settings: { foreground: highlight.heading, fontStyle: "bold" } },
    {
      scope: ["markup.underline.link", "string.other.link", "constant.other.reference.link"],
      settings: { foreground: highlight.link },
    },
    { scope: ["invalid", "invalid.illegal"], settings: { foreground: highlight.invalid } },
    { scope: ["meta.preprocessor"], settings: { foreground: highlight.meta } },
  ],
};
