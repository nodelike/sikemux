import { isImagePath } from "@shared/lib/imagePaths";
import { contextChip } from "@shared/chat/promptContext";
import { AGENT_URI, EARLIER_URI, WOKEN_URI } from "@shared/wire.generated";
import type { SentContext } from "@shared/chat/types";

type SentChip = { kind: "agent" | "chat" | "file" | "image" | "issue" | "pull"; label: string };

/** A sent message's text in order: its words, and the chips the draft wrote into it. */
type SentSpan = string | { written: string; chip: SentChip };

const AGENT_URIS = [AGENT_URI, WOKEN_URI];

/**
 * The chips a sent message's text holds, as the draft wrote them: the agents of `agents` it names with `@`,
 * its pictures as `[Image #n]`, its files by the `+` path written for them, and the named chats, issues and
 * pull requests it carried. `drawn` is the context those chips stand for, which needs no chip of its own.
 */
export function sentChips(
    text: string,
    attachments: readonly string[],
    context: readonly SentContext[],
    agents: readonly string[],
): { spans: SentSpan[]; drawn: Set<string> } {
    const found: { written: string; chip: SentChip; uri?: string }[] = agents.map((name) => ({
        written: `@${name}`,
        chip: { kind: "agent", label: `@${name}` },
    }));
    attachments.forEach((path, index) => {
        if (isImagePath(path)) {
            found.push({ written: `[Image #${index + 1}]`, chip: { kind: "image", label: `Image #${index + 1}` } });
            return;
        }
        const parts = path.split("/").filter(Boolean);
        const tag = parts.map((_, from) => `+${parts.slice(from).join("/")}`).find((written) => text.includes(written));
        if (tag) found.push({ written: tag, chip: { kind: "file", label: tag } });
    });
    for (const item of context) {
        const chip = contextChip(item);
        if (item.title.startsWith("@") && item.uri.startsWith(EARLIER_URI)) {
            found.push({ written: item.title, chip: { kind: "chat", label: item.title }, uri: item.uri });
        } else if (chip.kind && chip.number) {
            const sign = chip.kind === "pull" ? "!" : "#";
            found.push({
                written: `${sign}${chip.number}`,
                chip: { kind: chip.kind, label: `#${chip.number} ${chip.title}` },
                uri: item.uri,
            });
        }
    }
    const spans: SentSpan[] = [];
    const drawn = new Set<string>();
    let rest = text;
    for (;;) {
        let next: { at: number; mention: (typeof found)[number] } | undefined;
        for (const mention of found) {
            const at = rest.indexOf(mention.written);
            if (at < 0) continue;
            if (!next || at < next.at || (at === next.at && mention.written.length > next.mention.written.length)) next = { at, mention };
        }
        if (!next) break;
        if (next.at) spans.push(rest.slice(0, next.at));
        spans.push({ written: next.mention.written, chip: next.mention.chip });
        if (next.mention.uri) drawn.add(next.mention.uri);
        if (next.mention.chip.kind === "agent") {
            for (const item of context) if (AGENT_URIS.some((uri) => item.uri.startsWith(uri))) drawn.add(item.uri);
        }
        rest = rest.slice(next.at + next.mention.written.length);
    }
    if (rest) spans.push(rest);
    return { spans, drawn };
}
