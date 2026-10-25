/* A shared chat's host is the sharer's computer, which the page cannot ask anything of. */
export function invokeCommand<T>(
  command: string,
  _args?: Record<string, unknown>,
): Promise<T> {
  return Promise.reject(
    new Error(`${command} is not available on a shared chat's page`),
  );
}
