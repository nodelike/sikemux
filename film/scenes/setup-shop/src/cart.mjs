export function cartTotal(lines, { taxRate = 0, discountCents = 0 } = {}) {
  const subtotal = lines.reduce((sum, line) => sum + line.priceCents * line.quantity, 0);
  const taxed = Math.round((subtotal - discountCents) * (1 + taxRate));
  return Math.max(taxed, 0);
}

export function mergeLines(lines) {
  const merged = new Map();
  for (const line of lines) {
    const kept = merged.get(line.variantId);
    merged.set(line.variantId, kept ? { ...kept, quantity: kept.quantity + line.quantity } : { ...line });
  }
  return [...merged.values()];
}
