export { errorText, formatAge, formatAgo, formatBytes, formatDuration } from "../lib/format";

const timeOfDay = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const dayAndTime = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
const dateAndTime = new Intl.DateTimeFormat(undefined, { year: "numeric", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });

/** When something was said or done: the time if it was today, the day and time this year, and the year too before that. */
export function formatStamp(at: number | string, now = Date.now()): string {
    const date = new Date(at);
    if (Number.isNaN(date.getTime())) return "";
    const today = new Date(now);
    if (date.toDateString() === today.toDateString()) return timeOfDay.format(date);
    return (date.getFullYear() === today.getFullYear() ? dayAndTime : dateAndTime).format(date);
}
