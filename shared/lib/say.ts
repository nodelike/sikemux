/** Says an English sentence in the reader's language, with each `{name}` in it filled from `values`. */
export type Say = (english: string, values?: Record<string, string | number>) => string;

/** English, as every sentence is written. */
export const english: Say = (text, values) =>
    values ? text.replace(/\{(\w+)\}/g, (whole, name: string) => (name in values ? String(values[name]) : whole)) : text;
