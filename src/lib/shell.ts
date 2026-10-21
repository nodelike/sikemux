import { IS_WINDOWS } from "./platform";

export function posixQuote(value: string): string {
    return `'${value.replaceAll("'", `'"'"'`)}'`;
}

export function powershellQuote(value: string): string {
    return `'${value.replaceAll("'", "''")}'`;
}

/** A word for the local shell, quoted only when it needs to be. */
export function shellQuote(value: string): string {
    if (/^[A-Za-z0-9_./:=@+-]+$/.test(value)) return value;
    return IS_WINDOWS ? powershellQuote(value) : posixQuote(value);
}
