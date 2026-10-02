import { useContext, useMemo, type ReactNode } from "react";
import { CopyButton } from "../ui/CopyButton";
import { FileIcon } from "../ui/FileIcon";
import { hasPrimaryModifier } from "../lib/platform";
import { Markdown, type MarkdownComponents } from "../markdown/Markdown";
import type { MarkdownOptions } from "../markdown/types";
import { fencedDiff } from "./diff";
import { CodeTokens, fenceLanguage, useCodeTokens, useDiffTokens } from "./codeHighlight";
import { localImagePath, useImagePreview } from "./imagePreview";
import { ChatFileRef, useFileRef } from "./FileRef";
import { pathComponents, type PathGuess } from "./markdownPaths";
import { useLongTextFold } from "./longText";
import { ChatAgentContext, openLink } from "./chatAgent";
import { ChatImage } from "./ChatImage";
import { DiffText } from "./DiffView";
import { decodedFenceName } from "./transcript";

/* An agent writes an attached file back as a link to it. A picture beats its
   percent-encoded name, so show the picture whenever we can read it.

   A name the message only mentioned in passing arrives here too, marked as a
   guess. It is a file when the project has one by that name, and the words the
   agent wrote when it has not. */
function ChatLink({ href, guess, children }: { href: string; guess?: PathGuess; children?: ReactNode }) {
    const imagePath = localImagePath(href);
    const preview = useImagePreview(guess ? null : imagePath);
    const file = useFileRef(href);
    const agentId = useContext(ChatAgentContext).id;
    if (preview && imagePath) return <ChatImage src={preview} path={imagePath} />;
    if (file)
        return (
            <ChatFileRef
                refers={file.ref}
                state={file.state}
                label={children}
                className={guess === "code" ? "chat-file-ref code" : "chat-file-ref link"}
            />
        );
    if (guess === "code") return <code>{children}</code>;
    if (guess === "text") return <>{children}</>;
    return (
        <a
            href={href}
            onClick={(event) => {
                event.preventDefault();
                if (href) openLink(href, agentId, hasPrimaryModifier(event));
            }}>
            {children}
        </a>
    );
}

function ChatFence({ lang: info, text }: { lang?: string; text: string }) {
    const className = info ? `language-${info}` : undefined;
    const patch = useMemo(() => (text ? fencedDiff(text, info) : null), [text, info]);
    const tokens = useCodeTokens(text, patch ? null : fenceLanguage(info));
    // A patch in a fence is coloured the way the one in a tool call is, which
    // only happens at all when the fence says what file it is a patch to.
    const patchColours = useDiffTokens(patch, info);
    return (
        <pre>
            <CodeTitle info={info} text={text} />
            {patch ? (
                <code className={`${className ?? ""} chat-code-diff`}>
                    {patch.map((line, index) => (
                        <span className={`chat-diff-line${line.sign === "+" ? " add" : line.sign === "-" ? " del" : ""}`} key={index}>
                            <span className="chat-diff-sign">{line.sign}</span>
                            <DiffText line={line} tokens={patchColours?.get(line)} />
                        </span>
                    ))}
                </code>
            ) : tokens ? (
                <code className={className}>
                    <CodeTokens lines={tokens} />
                </code>
            ) : (
                <code className={className}>{text}</code>
            )}
        </pre>
    );
}

/* A fence says what file it quotes, when it says anything at all. The name is
   the file itself where the project has one; a bare language name is not. */
function CodeTitle({ info, text }: { info?: string; text: string }) {
    const name = info ? decodedFenceName(info) : "";
    const file = useFileRef(name || undefined);
    return (
        <span className="chat-code-title">
            {file ? (
                <ChatFileRef refers={file.ref} state={file.state} label={name} size={16} />
            ) : (
                name && (
                    <span className="chat-code-name">
                        <FileIcon name={name} size={16} />
                        {name}
                    </span>
                )
            )}
            <CopyButton className="chat-code-copy" value={text.replace(/\n$/, "")} label="code" size={12} />
        </span>
    );
}

function ChatTable({ children }: { children?: ReactNode }) {
    return (
        <div className="chat-table">
            <table>{children}</table>
        </div>
    );
}

const markdownComponents: MarkdownComponents = { link: ChatLink, fence: ChatFence, table: ChatTable, ...pathComponents(ChatLink) };
const AGENT_MARKDOWN: MarkdownOptions = { gfm: true, htmlAsText: false, fileLinks: true };
/* What a person typed shows its markup as the characters they typed. */
const TYPED_MARKDOWN: MarkdownOptions = { gfm: true, htmlAsText: true, fileLinks: true };

function LiveMarkdown({ text, live, typed = false }: { text: string; live: boolean; typed?: boolean }) {
    return <Markdown text={text} options={typed ? TYPED_MARKDOWN : AGENT_MARKDOWN} live={live} components={markdownComponents} />;
}

export function FoldedMarkdown({ id, text, live, typed = false }: { id: string; text: string; live: boolean; typed?: boolean }) {
    const { cut, expand } = useLongTextFold(id, text, live);
    if (!cut) return <LiveMarkdown text={text} live={live} typed={typed} />;
    return (
        <>
            <LiveMarkdown text={cut.head} live={false} typed={typed} />
            <button type="button" className="chat-show-rest" onClick={expand}>
                Show the rest — {Math.round(cut.hidden / 1000)}k more characters
            </button>
        </>
    );
}
