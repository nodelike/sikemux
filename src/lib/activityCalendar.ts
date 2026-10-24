const DAY_MS = 86_400_000;

/** Today as a local calendar day counted from the Unix epoch, the unit the activity summary uses. */
export function localDay(now: Date = new Date()): number {
    return Math.floor((now.getTime() - now.getTimezoneOffset() * 60_000) / DAY_MS);
}

/** 1970-01-01 was a Thursday; weeks here start on Monday. */
export function weekday(day: number): number {
    return (((day + 3) % 7) + 7) % 7;
}

export function dayDate(day: number): Date {
    return new Date(day * DAY_MS);
}

export interface CalendarColumn {
    /** Seven days, Monday first. Days after today are null. */
    days: (number | null)[];
    /** Set on the first column of each month that has room for its label. */
    month?: string;
}

const MONTH = new Intl.DateTimeFormat(undefined, { month: "short", timeZone: "UTC" });

export function calendarColumns(today: number, weeks = 53): CalendarColumn[] {
    const start = today - weekday(today) - (weeks - 1) * 7;
    const columns: CalendarColumn[] = [];
    let lastMonth = -1;
    for (let week = 0; week < weeks; week += 1) {
        const days = Array.from({ length: 7 }, (_, index) => {
            const day = start + week * 7 + index;
            return day <= today ? day : null;
        });
        const first = dayDate(start + week * 7);
        const month = first.getUTCMonth();
        const column: CalendarColumn = { days };
        if (month !== lastMonth) {
            if (week <= weeks - 3) column.month = MONTH.format(first);
            lastMonth = month;
        }
        columns.push(column);
    }
    if (columns[0]?.month && columns[1] && columns.slice(1, 3).some((column) => column.month)) delete columns[0].month;
    return columns;
}
