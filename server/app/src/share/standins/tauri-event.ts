/* Stands in for @tauri-apps/api/event in a browser, where nothing is ever emitted. */
export function listen(): Promise<() => void> {
  return Promise.resolve(() => undefined);
}
