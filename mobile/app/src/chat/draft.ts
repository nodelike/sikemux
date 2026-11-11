import type { ConnectionLike } from '@sikemux/native';
import { File } from 'expo-file-system';

import type { ChatRows } from '@shared/chat/rows';
import { CHAT_DRAFT, type DraftExtra } from '@shared/wire.generated';

import { callHost } from '@/core/hostCalls';
import type { DraftStore } from './kept';
import { IMAGE, tagPictures, untagPicture } from './imageTags';
import { editDraft, keptTokens, outgoing, tokensOf, trackedOf, type DraftToken } from './mentions';
import { fitting, pathsOf, type Attachment } from './uploads';
import { contextChipsOf, withContext, type ContextChip } from '@/stash/words';

/** How long typing rests before the draft is saved on the host. */
const DRAFT_SAVE_MS = 600;

const PICTURE = /\.(jpe?g|png|gif|webp|heic|heif)$/i;

/** A file the host's draft holds, known here only by where the host keeps it. */
function hostFile(path: string): Attachment {
  return {
    id: `draft:${path}`,
    kind: PICTURE.test(path) ? 'image' : 'file',
    name: path.split('/').pop() || path,
    mime: 'application/octet-stream',
    uri: path,
    size: 0,
    path,
  };
}

/** What a message's chips send besides its words. */
export type Named = { paths: string[]; context: ContextChip[] };

type Host = {
  /** Where writing not saved on the host yet is kept, so it outlives the app quitting. */
  store?: DraftStore;
  /** The chat's draft as the host last sent it. */
  held: () => ChatRows['draft'];
  /** The connection, while it is open and the chat attached over it. */
  live: () => ConnectionLike | undefined;
  emit: () => void;
};

/** What the composer holds for the chat's next message, saved as the chat's own draft on the host. */
export class Draft {
  text = '';
  /** Picked for the next message. */
  attachments: Attachment[] = [];
  /** Links and text the draft carries, as the host keeps them: shown as chips, sent as words. */
  chips: ContextChip[] = [];
  /** The agents, files, chats and issues the draft names, over the words that name them. */
  tokens: DraftToken[] = [];
  /** The issues and pull requests the draft's chips name, by chip. */
  private tracked: Record<string, ContextChip> = {};
  /** The host's revision of this chat's draft that the composer last took in. */
  private rev: number | undefined;
  /** What the host's draft carries that this build does not read; saved back as it came. */
  private extra: Record<string, unknown> = {};
  /** Changes made here since the draft was last saved, which the host's draft must not replace. */
  private edits = 0;
  private saving: ReturnType<typeof setTimeout> | undefined;

  constructor(
    private readonly agentId: string,
    private readonly host: Host,
  ) {
    const unsaved = host.store?.load();
    if (!unsaved) return;
    this.text = unsaved.text;
    this.attachments = unsaved.attachments.filter((attachment) => attachment.path || new File(attachment.uri).exists);
    const { tokens, tracked, ...extra } = unsaved.extra;
    this.extra = extra;
    this.tokens = tokensOf(unsaved.text, tokens);
    this.tracked = trackedOf(tracked);
    this.chips = unsaved.chips ?? [];
    this.rev = unsaved.rev;
    this.edits = 1;
  }

  /** Answers whether the draft named anything before, so its chips must be drawn again. */
  write(text: string): boolean {
    const named = this.tokens.length > 0;
    const tagged = this.tokens.filter((token) => token.id.startsWith(IMAGE)).map((token) => token.id);
    ({ text: this.text, tokens: this.tokens } = editDraft(this.text, text, this.tokens));
    // Deleting a picture's tag takes the picture off the message, as on the Mac.
    for (const id of tagged) if (!this.tokens.some((token) => token.id === id)) this.detach(id.slice(IMAGE.length));
    this.edited();
    return named;
  }

  /** Writes what was picked from the mention menu over the word from `start` that asked for it, as a chip when it is one. */
  mention(start: number, pick: { written: string; id?: string; label?: string; context?: ContextChip }) {
    const before = this.text.slice(0, start);
    const kept = this.tokens.filter((token) => token.end <= start);
    this.tokens = pick.id
      ? [...kept, { start, end: start + pick.written.length, id: pick.id, ...(pick.label ? { label: pick.label } : {}) }]
      : kept;
    const context = pick.context;
    if (context && !pick.id && !this.chips.some((chip) => chip.uri === context.uri)) this.chips = [...this.chips, context];
    this.text = pick.written ? `${before}${pick.written} ` : before;
    this.edited();
  }

  /** Keeps what the issue or pull request chip `id` sends, once the host has read it. */
  track(id: string, context: ContextChip) {
    if (!this.tokens.some((token) => token.id === id)) return;
    this.tracked = { ...this.tracked, [id]: context };
    this.edited();
  }

  /**
   * Adds what was picked, leaving out what the host would refuse, and names each new picture in the words at `caret`;
   * answers why some was left out.
   */
  attach(picked: Attachment[], caret?: number): string | null {
    const { fits, notice } = fitting(this.attachments.length, picked);
    ({ text: this.text, tokens: this.tokens } = tagPictures(this.text, this.tokens, this.attachments.length, fits, caret));
    this.attachments = [...this.attachments, ...fits];
    this.edited();
    return notice;
  }

  removeChip(uri: string) {
    this.chips = this.chips.filter((chip) => chip.uri !== uri);
    this.edited();
  }

  /** Answers whether it could, which it cannot while a file is going up. */
  removeAttachment(id: string): boolean {
    if (this.uploading()) return false;
    this.detach(id);
    this.edited();
    return true;
  }

  /** Takes the file `id` off the message with its tags, counting the later pictures down. */
  private detach(id: string) {
    this.attachments = this.attachments.filter((attachment) => attachment.id !== id);
    ({ text: this.text, tokens: this.tokens } = untagPicture(this.text, this.tokens, id, this.attachments));
  }

  uploading() {
    return this.attachments.some((attachment) => attachment.upload === 'sending');
  }

  patch(id: string, change: Partial<Attachment>) {
    this.attachments = this.attachments.map((attachment) => (attachment.id === id ? { ...attachment, ...change } : attachment));
    this.host.emit();
  }

  /** Takes the files and chips out for the message `text`, answering it with its text chips written in. */
  takeOut(text: string): { text: string; files: Attachment[]; named: Named } {
    const files = this.attachments;
    this.attachments = [];
    const named = outgoing(this.text, this.tokens, this.tracked);
    named.context.push(...this.chips.filter((chip) => !chip.text));
    const written = withContext(
      text,
      this.chips.filter((chip) => chip.text),
    );
    this.chips = [];
    this.tokens = [];
    this.tracked = {};
    this.edited();
    return { text: written, files, named };
  }

  /** Puts `texts` and `files` back before what the composer holds now. */
  putBack(texts: readonly string[], files: readonly Attachment[]) {
    this.text = [...texts, this.text].filter(Boolean).join('\n\n');
    this.attachments = [...files, ...this.attachments];
  }

  /** Empties the draft once the host holds it elsewhere, such as in the stash. */
  clear() {
    this.text = '';
    this.attachments = [];
    this.chips = [];
    this.tokens = [];
    this.tracked = {};
  }

  /** Saves the draft on the host now, answering whether everything written here is saved. */
  async saveNow(): Promise<boolean> {
    clearTimeout(this.saving);
    await this.save();
    return !this.edits;
  }

  /** The draft changed here: it is saved on the host once typing rests. */
  edited() {
    this.edits += 1;
    clearTimeout(this.saving);
    this.saving = setTimeout(() => {
      this.keep();
      void this.save();
    }, DRAFT_SAVE_MS);
  }

  /** Writes down what is not saved on the host yet, so it outlives the app quitting. */
  keep() {
    if (!this.edits) return;
    this.host.store?.save({
      text: this.text,
      attachments: this.attachments.filter((attachment) => !attachment.upload),
      chips: this.chips,
      extra: { ...this.extra, ...this.namedExtra() },
      rev: this.rev,
    });
  }

  /**
   * Saves the draft as the chat's own on the host, naming the revision last taken in, so the host keeps
   * whatever it put in the draft since. A save that fails waits for the next change or attach.
   */
  async save() {
    const connection = this.host.live();
    if (!this.edits || !connection) return;
    const edits = this.edits;
    const paths = pathsOf(this.attachments);
    const { attachments: _files, context: _chips, ...rest } = this.extra;
    const draft = {
      key: `${CHAT_DRAFT}${this.agentId}`,
      text: this.text,
      extra: {
        ...rest,
        ...(paths.length ? { attachments: paths } : {}),
        ...(this.chips.length ? { context: this.chips } : {}),
        ...this.namedExtra(),
      },
      ...(this.rev === undefined ? {} : { rev: this.rev }),
    };
    try {
      await callHost(connection, 'setDraft', draft);
    } catch {
      return;
    }
    if (this.edits !== edits) return;
    this.edits = 0;
    this.host.store?.clear();
    this.takeIn();
  }

  /** The draft's chips as every app keeps them beside its text. */
  private namedExtra() {
    const tracked = Object.entries(this.tracked).filter(([id]) => this.tokens.some((token) => token.id === id));
    return {
      ...(this.tokens.length ? { tokens: keptTokens(this.text, this.tokens) } : {}),
      ...(tracked.length ? { tracked: Object.fromEntries(tracked) } : {}),
    };
  }

  /** Takes in the host's draft, unless changes made here are still to be saved over it. */
  takeIn() {
    if (this.edits) return;
    const host = this.host.held();
    if (host?.rev === this.rev && host) return;
    const extra: DraftExtra = host?.extra ?? {};
    const { attachments: paths = [], context, tokens, tracked, ...rest } = extra;
    const kept = new Map(this.attachments.flatMap((attachment) => (attachment.path ? [[attachment.path, attachment] as const] : [])));
    this.rev = host?.rev;
    this.extra = rest;
    this.text = host?.text ?? '';
    this.tokens = tokensOf(this.text, tokens);
    this.tracked = trackedOf(tracked);
    this.chips = contextChipsOf(context);
    this.attachments = [
      ...paths.map((path) => kept.get(path) ?? hostFile(path)),
      ...this.attachments.filter((attachment) => !attachment.path),
    ];
    this.host.emit();
  }
}
