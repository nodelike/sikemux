/** How deep a row is indented at most; rows below that stay at this depth and name their lead. */
export const DEEPEST = 3;

/** What sits at one level to a row's left: nothing, a spine passing by, a tick with the spine going on, or a corner. */
export type Line = 'blank' | 'through' | 'tick' | 'corner';

export type Leaf = { id: string; lead?: string };

/** One row of an agent tree, laid out as the Mac's rail lays it out (`app/src/agents/tree.rs`). */
export type Placed<T> = {
  item: T;
  depth: number;
  lines: Line[];
  /** Its lead's id, when the row does not hang under its lead's row. */
  leadApart?: string;
  /** Rows hang under it right below, so a line drops from its mark. */
  open: boolean;
  /** How many hang below it at any depth. */
  below: number;
};

/**
 * `items` laid out as trees by each one's lead, every root at `root` depth, siblings in the order they come. One whose
 * lead is not among them is a root and names its lead; the rows under a lead in `folded` are left out.
 */
export function tree<T>(items: readonly T[], leaf: (item: T) => Leaf, root: number, folded: ReadonlySet<string>): Placed<T>[] {
  const leaves = items.map(leaf);
  const at = new Map(leaves.map((row, index) => [row.id, index]));
  const parent = leaves.map((row, index) => {
    const lead = row.lead === undefined ? undefined : at.get(row.lead);
    return lead === index ? undefined : lead;
  });
  const children: number[][] = leaves.map(() => []);
  parent.forEach((lead, index) => {
    if (lead !== undefined) children[lead].push(index);
  });
  const below = leaves.map(() => 0);
  const placed = leaves.map(() => false);
  const order: [number, number][] = [];
  const walk = (index: number, depth: number): number => {
    placed[index] = true;
    order.push([index, depth]);
    let count = 0;
    for (const child of children[index]) if (!placed[child]) count += 1 + walk(child, depth + 1);
    below[index] = count;
    return count;
  };
  leaves.forEach((_, index) => {
    if (parent[index] === undefined) walk(index, 0);
  });
  leaves.forEach((_, index) => {
    if (!placed[index]) walk(index, 0);
  });
  const shown: [number, number][] = [];
  let hiddenBelow: number | undefined;
  for (const [index, depth] of order) {
    if (hiddenBelow !== undefined && depth > hiddenBelow) continue;
    hiddenBelow = undefined;
    if (below[index] > 0 && folded.has(leaves[index].id)) hiddenBelow = depth;
    shown.push([index, depth]);
  }
  const depths = shown.map(([, depth]) => Math.min(root + depth, DEEPEST));
  const lines = linesOf(depths);
  return shown.map(([index, depth], row) => {
    const drawn = depths[row];
    const deeper = root + depth > drawn;
    return {
      item: items[index],
      depth: drawn,
      lines: lines[row],
      leadApart: parent[index] !== undefined && !deeper ? undefined : leaves[index].lead,
      open: (depths[row + 1] ?? 0) > drawn,
      below: below[index],
    };
  });
}

/** The lines to the left of each row of a tree drawn at `depths`, top to bottom. */
export function linesOf(depths: readonly number[]): Line[][] {
  const later: boolean[] = [];
  const lines: Line[][] = depths.map(() => []);
  for (let index = depths.length - 1; index >= 0; index--) {
    const depth = depths[index];
    lines[index] = Array.from({ length: depth }, (_, level) => {
      const last = level + 1 === depth;
      if (later[level]) return last ? 'tick' : 'through';
      return last ? 'corner' : 'blank';
    });
    later.length = Math.min(later.length, depth);
    if (depth > 0) later[depth - 1] = true;
  }
  return lines;
}

/** The ids of the rows that hang, at any depth, above one of `from`. */
export function above(leaves: readonly Leaf[], from: Iterable<string>): Set<string> {
  const leadOf = new Map(leaves.flatMap((row) => (row.lead ? [[row.id, row.lead] as const] : [])));
  const leads = new Set<string>();
  for (const start of from) {
    let at = leadOf.get(start);
    while (at !== undefined && !leads.has(at)) {
      leads.add(at);
      at = leadOf.get(at);
    }
  }
  return leads;
}

/** The top of the tree each row hangs in, by its id. */
export function topOf(leaves: readonly Leaf[]): Map<string, string> {
  const byId = new Map(leaves.map((row) => [row.id, row]));
  return new Map(
    leaves.map((row) => {
      let top = row;
      for (let step = 0; step < leaves.length; step++) {
        const lead = top.lead === undefined ? undefined : byId.get(top.lead);
        if (!lead) break;
        top = lead;
      }
      return [row.id, top.id];
    }),
  );
}
