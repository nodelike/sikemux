/** An answer to a question as one text: what was typed, the files attached to it, then any context. */
export function answerText(text: string, paths: readonly string[], context: readonly { title: string; text: string }[] = []): string {
    return [
        text,
        ...(paths.length > 0 ? [`Attached:\n${paths.map((path) => `- ${path}`).join("\n")}`] : []),
        ...context.map((item) => `${item.title}:\n${item.text}`),
    ]
        .filter(Boolean)
        .join("\n\n");
}
