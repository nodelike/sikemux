import type { WhyChip } from "../wire.generated";
import { english, type Say } from "../lib/say";

/** Where an answer's `why` links point; the core checks each one and sends its chip with the message. */
export const WHY_URI = "sikemux://why/";

export type WhyIcon = "IconMessage" | "IconCommit" | "IconCheck" | "IconBook" | "IconMerge";

export type WhyWording = {
    say?: Say;
    /** The language the dates are written in; the device's when absent. */
    locale?: string;
    hour12?: boolean;
    now?: number;
};

/** The chip the core checked for `href` among a message's, or none for a link it did not check. */
export function whyChip(chips: WhyChip[] | undefined, href: string): WhyChip | undefined {
    return href.startsWith(WHY_URI) ? chips?.find((chip) => chip.href === href) : undefined;
}

/** A `why` chip's icon and words as the Mac draws them: what it is, then when. */
export function whyWords(chip: WhyChip, wording: WhyWording = {}): { icon: WhyIcon; key: string; title: string } {
    const say = wording.say ?? english;
    const day = (time: boolean) => (chip.at === undefined ? "" : saidAt(chip.at, time, wording));
    switch (chip.kind) {
        case "said":
            return { icon: "IconMessage", key: say("your message"), title: day(true) };
        case "change":
            return {
                icon: "IconCommit",
                key: say("the change"),
                title: chip.landed ? say("landed {day}", { day: day(false) }) : day(false),
            };
        case "call":
            return { icon: "IconCheck", key: say("your answer"), title: day(false) };
        case "rule":
            return { icon: "IconBook", key: say("your rule"), title: say("said {day}", { day: day(false) }) };
        case "decision":
            return { icon: "IconMerge", key: say("decided"), title: day(false) };
        default:
            return { icon: "IconMessage", key: say("source"), title: day(false) };
    }
}

/** When something happened, as a person says it looking back: the time today, the weekday this week, else the date. */
function saidAt(at: number, time: boolean, { say = english, locale, hour12, now = Date.now() }: WhyWording): string {
    const when = new Date(at);
    const midnight = (date: Date) => new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
    const days = Math.round((midnight(new Date(now)) - midnight(when)) / 86_400_000);
    const clock = when.toLocaleTimeString(locale, { hour: "numeric", minute: "2-digit", hour12 });
    const weekday = when.toLocaleDateString(locale, { weekday: "short" });
    if (days <= 0) return time ? clock : say("today");
    if (days <= 6) return time ? `${weekday} ${clock}` : weekday;
    return when.toLocaleDateString(locale, { month: "short", day: "numeric" });
}
