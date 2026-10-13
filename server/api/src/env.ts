/** Whether `value` is a TCP or UDP port number. */
export function isPort(value: number): boolean {
  return Number.isInteger(value) && value >= 1 && value <= 65535;
}

/**
 * Reads settings from `env`, trimmed, adding what is wrong with each to `problems`, so one
 * failed start names every problem at once. A blank setting counts as unset.
 */
export function settingsFrom(env: NodeJS.ProcessEnv, problems: string[]) {
  const optional = (name: string): string | undefined =>
    env[name]?.trim() || undefined;

  /** The setting, or `fallback`; without a fallback, unset is a problem. */
  const text = (name: string, fallback?: string): string => {
    const value = optional(name) ?? fallback;
    if (value === undefined) problems.push(`${name} is not set`);
    return value ?? "";
  };

  const oneOf = <T extends string>(
    name: string,
    values: readonly T[],
    fallback: T,
  ): T => {
    const value = text(name, fallback) as T;
    if (!values.includes(value))
      problems.push(`${name} is not one of ${values.join(", ")}`);
    return value;
  };

  const port = (name: string, fallback: string): number => {
    const value = Number(text(name, fallback));
    if (!isPort(value)) problems.push(`${name} is not a port number`);
    return value;
  };

  /** "1" for on, "0" for off. */
  const flag = (name: string, fallback: boolean): boolean => {
    const value = text(name, fallback ? "1" : "0");
    if (value !== "0" && value !== "1") problems.push(`${name} is not 0 or 1`);
    return value === "1";
  };

  return { problems, optional, text, oneOf, port, flag };
}
