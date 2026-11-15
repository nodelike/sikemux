#!/usr/bin/env node
// Exits 0 when the first release version comes after the second: 0.6.1 after 0.6.1-nightly.30.
const [a, b] = process.argv.slice(2);
const order = (version) => {
  const match = /^(\d+)\.(\d+)\.(\d+)(?:-nightly\.(\d+))?$/.exec(version ?? "");
  if (!match) throw new Error(`${version} is not a release version`);
  return [
    +match[1],
    +match[2],
    +match[3],
    match[4] === undefined ? Infinity : +match[4],
  ];
};
const [x, y] = [order(a), order(b)];
const later = x.findIndex((part, i) => part !== y[i]);
process.exit(later >= 0 && x[later] > y[later] ? 0 : 1);
