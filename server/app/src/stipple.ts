let mask: string | undefined;

/** A disc made of dots, as a CSS image to mask a seat nobody sits in. */
export function stippledDisc() {
  mask ??= `url(${draw()})`;
  return mask;
}

function draw() {
  const size = 36;
  const scale = 4;
  const gap = 1.8;
  const middle = size / 2;
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = size * scale;
  const pen = canvas.getContext("2d");
  if (!pen) return "";
  const random = sequence(11);
  const placed: [number, number][] = [];
  const put = (x: number, y: number, apart: number) => {
    if (placed.some(([px, py]) => (px - x) ** 2 + (py - y) ** 2 < apart ** 2))
      return;
    placed.push([x, y]);
    pen.globalAlpha = 0.72 + 0.28 * random();
    pen.beginPath();
    pen.arc(x * scale, y * scale, (0.5 + 0.42 * random()) * scale, 0, 7);
    pen.fill();
  };
  for (let i = 0; i < 260; i++) {
    const angle = random() * 2 * Math.PI;
    const reach = middle - 1.2;
    put(
      middle + Math.cos(angle) * reach,
      middle + Math.sin(angle) * reach,
      gap * 0.85,
    );
  }
  for (let i = 0; i < size * 60; i++) {
    const angle = random() * 2 * Math.PI;
    const reach = Math.sqrt(random()) * (middle - 1.5);
    const x = (Math.cos(angle) * reach) / size;
    const y = (Math.sin(angle) * reach) / size;
    const shade = Math.max(0, 0.35 + x * 0.6 + y);
    put(middle + x * size, middle + y * size, gap * (0.8 + 1.15 * shade));
  }
  return canvas.toDataURL();
}

function sequence(seed: number) {
  let state = seed;
  return () => {
    state = (state * 1664525 + 1013904223) >>> 0;
    return state / 2 ** 32;
  };
}
