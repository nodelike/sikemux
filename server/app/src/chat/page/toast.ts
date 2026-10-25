/* The page shows no notices: a shared chat is read-only, so nothing it does needs reporting. */
export function notify(
  _kind: "info" | "error" | "success",
  _text: string,
): void {}

export function reportError(_label: string): (err: unknown) => void {
  return () => {};
}

export function swallow(_label: string): (err: unknown) => void {
  return () => {};
}
