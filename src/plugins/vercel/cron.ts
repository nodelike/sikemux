const DAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

const pad = (value: string) => value.padStart(2, "0");
const isNumber = (field: string) => /^\d+$/.test(field);

/** A cron schedule in words, for the shapes people write most; anything else is shown as written. Vercel runs crons in UTC. */
export function describeCron(schedule: string): string {
    const fields = schedule.trim().split(/\s+/);
    if (fields.length !== 5) return schedule;
    const [minute, hour, dayOfMonth, month, dayOfWeek] = fields as [string, string, string, string, string];
    const everyDay = dayOfMonth === "*" && month === "*";
    if (minute === "*" && hour === "*" && everyDay && dayOfWeek === "*") return "Every minute";
    const step = /^\*\/(\d+)$/.exec(minute);
    if (step && hour === "*" && everyDay && dayOfWeek === "*") return `Every ${step[1]} minutes`;
    if (isNumber(minute) && hour === "*" && everyDay && dayOfWeek === "*") return `Every hour at :${pad(minute)}`;
    const hourStep = /^\*\/(\d+)$/.exec(hour);
    if (isNumber(minute) && hourStep && everyDay && dayOfWeek === "*") return `Every ${hourStep[1]} hours at :${pad(minute)}`;
    if (!isNumber(minute) || !isNumber(hour)) return schedule;
    const time = `${pad(hour)}:${pad(minute)} UTC`;
    if (everyDay && dayOfWeek === "*") return `Every day at ${time}`;
    if (everyDay && dayOfWeek === "1-5") return `Weekdays at ${time}`;
    if (everyDay && isNumber(dayOfWeek) && Number(dayOfWeek) <= 7) return `Every ${DAYS[Number(dayOfWeek) % 7]} at ${time}`;
    if (isNumber(dayOfMonth) && month === "*" && dayOfWeek === "*") return `Monthly on day ${dayOfMonth} at ${time}`;
    return schedule;
}
