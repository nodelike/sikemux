/** Which letters a TrueType font draws, and how wide each is. */
export class Font {
  private readonly view: DataView;
  private readonly cmap: number;
  private readonly hmtx: number;
  private readonly wide: number;
  private readonly unitsPerEm: number;
  private readonly glyphs = new Map<number, number>();

  constructor(bytes: Uint8Array) {
    this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const tables = new Map<string, number>();
    for (let i = 0; i < this.view.getUint16(4); i++) {
      const at = 12 + i * 16;
      const tag = Buffer.from(bytes.subarray(at, at + 4)).toString("latin1");
      tables.set(tag, this.view.getUint32(at + 8));
    }
    const table = (tag: string) => {
      const at = tables.get(tag);
      if (at === undefined) throw new Error(`the font has no ${tag} table`);
      return at;
    };
    this.unitsPerEm = this.view.getUint16(table("head") + 18);
    this.wide = this.view.getUint16(table("hhea") + 34);
    this.hmtx = table("hmtx");
    this.cmap = this.unicodeMap(table("cmap"));
  }

  /** The font's map of the basic plane, in cmap format 4. */
  private unicodeMap(cmap: number): number {
    for (let i = 0; i < this.view.getUint16(cmap + 2); i++) {
      const at = cmap + this.view.getUint32(cmap + 8 + i * 8);
      if (this.view.getUint16(at) === 4) return at;
    }
    throw new Error("the font has no cmap of format 4");
  }

  private glyph(code: number): number {
    const known = this.glyphs.get(code);
    if (known !== undefined) return known;
    const glyph = code > 0xffff ? 0 : this.look(code);
    this.glyphs.set(code, glyph);
    return glyph;
  }

  private look(code: number): number {
    const { view, cmap } = this;
    const twice = view.getUint16(cmap + 6);
    const ends = cmap + 14;
    const starts = ends + twice + 2;
    const deltas = starts + twice;
    const offsets = deltas + twice;
    for (let i = 0; i < twice; i += 2) {
      if (view.getUint16(ends + i) < code) continue;
      const start = view.getUint16(starts + i);
      if (start > code) return 0;
      const delta = view.getUint16(deltas + i);
      const offset = view.getUint16(offsets + i);
      if (offset === 0) return (code + delta) & 0xffff;
      const glyph = view.getUint16(offsets + i + offset + (code - start) * 2);
      return glyph && (glyph + delta) & 0xffff;
    }
    return 0;
  }

  has(code: number): boolean {
    return this.glyph(code) !== 0;
  }

  width(text: string, size: number, spacing = 0): number {
    let units = 0;
    let letters = 0;
    for (const letter of text) {
      const glyph = this.glyph(letter.codePointAt(0) ?? 0);
      units += this.view.getUint16(
        this.hmtx + Math.min(glyph, this.wide - 1) * 4,
      );
      letters += 1;
    }
    return (units / this.unitsPerEm + letters * spacing) * size;
  }

  /**
   * The text less its emoji and other signs the font lacks, or null when it is written in letters
   * the font lacks.
   */
  drawable(text: string): string | null {
    let out = "";
    for (const letter of text) {
      if (this.has(letter.codePointAt(0) ?? 0)) out += letter;
      else if (
        /[\p{L}\p{N}\p{M}]/u.test(letter) &&
        !/[\p{Variation_Selector}\u20e3]/u.test(letter)
      )
        return null;
    }
    return out.replace(/\s+/gu, " ").trim() || null;
  }
}
