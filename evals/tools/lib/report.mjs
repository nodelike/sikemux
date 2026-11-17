export function report(summary) {
  const variants = summary.variants;
  const trials = Math.max(...variants.map((variant) => variant.trials ?? 1));
  const rate = (rows) => {
    if (!rows.length) return '-';
    const all = rows.filter((row) => row.pass).length;
    if (trials === 1) return `${Math.round((100 * all) / rows.length)}% ${all}/${rows.length}`;
    const mean = rows.reduce((sum, row) => sum + (row.rate ?? Number(row.pass)), 0) / rows.length;
    return `${Math.round(100 * mean)}% (all ${all}/${rows.length})`;
  };
  const width = Math.max(trials === 1 ? 14 : 22, ...variants.map((variant) => variant.label.length + 2));
  const table = (title, keysOf) => {
    const keys = [...new Set(variants.flatMap((variant) => variant.cases.flatMap(keysOf)))].sort();
    console.log(`\n${title.padEnd(26)}${variants.map((variant) => variant.label.padEnd(width)).join('')}`);
    for (const key of keys) {
      const cells = variants.map((variant) => rate(variant.cases.filter((row) => keysOf(row).includes(key))).padEnd(width));
      console.log(`${key.padEnd(26)}${cells.join('')}`);
    }
  };

  if (trials > 1) console.log(`\n${trials} trials of each case: pass is the mean over trials, (all) counts the cases that passed every trial`);
  console.log(`\n${'model'.padEnd(26)}${'pass'.padEnd(width)}${'cost'.padEnd(10)}${'tokens in'.padEnd(14)}${'out'.padEnd(10)}per run`);
  for (const variant of variants) {
    const cost = variant.cases.reduce((sum, row) => sum + row.cost, 0);
    const input = variant.cases.reduce((sum, row) => sum + row.tokens.input, 0);
    const output = variant.cases.reduce((sum, row) => sum + row.tokens.output, 0);
    const mark = variant.cases.some((row) => row.costEstimated) ? '~' : '';
    const runs = variant.cases.reduce((sum, row) => sum + (row.runs?.length ?? 1), 0);
    const cached = variant.cases.reduce((sum, row) => sum + (row.tokens.cached ?? 0), 0);
    const turns = variant.cases.reduce((sum, row) => sum + (row.turns ?? 0), 0);
    const seconds = variant.cases.reduce((sum, row) => sum + (row.seconds ?? 0), 0);
    const each = runs ? `${mark}$${(cost / runs).toFixed(3)}, ${Math.round(input / runs / 1000)}k in (${Math.round(cached / runs / 1000)}k cached), ${(turns / runs).toFixed(1)} turns, ${Math.round(seconds / runs)} s` : '-';
    console.log(`${variant.label.padEnd(26)}${rate(variant.cases).padEnd(width)}${`${mark}$${cost.toFixed(2)}`.padEnd(10)}${input.toLocaleString().padEnd(14)}${output.toLocaleString().padEnd(10)}${each}`);
  }
  if (variants.some((variant) => variant.cases.some((row) => row.costEstimated))) {
    console.log('~ estimated from tokens at the provider price in lib/omp.mjs; the judge is counted at its real cost');
  }
  table('category', (row) => [row.category]);
  table('tool', (row) => row.tools);
  for (const variant of variants) {
    const failing = variant.cases.filter((row) => !row.pass);
    if (!failing.length) continue;
    console.log(`\nfailing on ${variant.label}:`);
    for (const row of failing) {
      const passed = row.runs ? ` (passed ${row.runs.filter((run) => run.pass).length}/${row.runs.length})` : '';
      const reasons = [...new Set(row.failures.map((failure) => failure.why))];
      console.log(`  ${row.id}${passed}: ${reasons.join('; ')}`);
    }
  }
}
