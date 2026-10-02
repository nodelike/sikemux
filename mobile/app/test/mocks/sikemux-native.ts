const TAGS = ['Refused', 'WrongCode', 'Connection', 'Invalid', 'Outdated', 'Unpaired'] as const;
type Tag = (typeof TAGS)[number];

class FakeMobileError extends Error {
  constructor(
    readonly tag: Tag,
    readonly inner: { message: string; macIsOlder?: boolean },
  ) {
    super(`MobileError.${tag}`);
  }
}

function variant(tag: Tag) {
  return {
    new: (inner: { message: string; macIsOlder?: boolean } = { message: tag }) => new FakeMobileError(tag, inner),
    // By tag rather than class, so values made before `vi.resetModules` still match.
    instanceOf: (error: unknown): error is FakeMobileError =>
      (error as FakeMobileError | undefined)?.tag === tag && 'inner' in (error as object),
  };
}

export const MobileError = Object.fromEntries(TAGS.map((tag) => [tag, variant(tag)])) as Record<Tag, ReturnType<typeof variant>>;

function notMocked(name: string): never {
  throw new Error(`@sikemux/native ${name} runs Rust; vi.mock('@sikemux/native') in the test that needs it`);
}

export const newDeviceKey = (): ArrayBuffer => new Uint8Array(32).buffer;
export const parsePairingLink = (_text: string): undefined => undefined;

export class Device {
  constructor() {
    notMocked('Device');
  }
}

export enum ChatState {
  Starting,
  Ready,
  Stopped,
}

export enum SessionKind {
  Terminal,
  Task,
}

export enum BuildChannel {
  Dev,
  Nightly,
  Stable,
}

/** A tagged value shaped like the generated enums': `inner` holds the fields, `instanceOf` checks the tag. */
function tagged<Inner>(tag: string) {
  class Variant {
    readonly tag = tag;
    constructor(readonly inner: Inner) {}
    static new(inner: Inner) {
      return new Variant(inner);
    }
    static instanceOf(value: unknown): value is Variant {
      return (value as Variant | undefined)?.tag === tag;
    }
  }
  return Variant;
}

export const CoreEvent = {
  Chat: tagged<{ agentId: string; seq: bigint; eventJson: string }>('Chat'),
  View: tagged<{ view: unknown }>('View'),
  Exited: tagged<{ session: bigint; code?: number; signal?: string; killed: boolean }>('Exited'),
};

export const ChatAttachment = {
  Live: tagged<Record<string, unknown>>('Live'),
  Resumed: tagged<{ eventsJson: string; mark: { feed: string; seq: bigint } }>('Resumed'),
  Missing: tagged<Record<string, never>>('Missing'),
  Restart: tagged<Record<string, never>>('Restart'),
};
