import type { FindAnswer, FindAt, FindHit } from '@shared/wire.generated';

import type { Finding, Range } from './found';
import type { ChatSession } from './session';

type FindState = {
  text: string;
  matchCase: boolean;
  wholeWord: boolean;
  /** `3/41…` while the host goes on counting, or why there is nothing to step to. */
  label: string;
  /** What the rows draw marked, or null while nothing is looked for. */
  finding: Finding | null;
};

/** Places found the host is asked for at a time. */
const HITS = 200;
/** Counting stops here, and the bar says 999+. */
const MOST_COUNTED = 1000;
/** How long typing rests before the host is asked. */
const TYPING_MS = 200;

/**
 * Find in a chat, as the Mac's find bar does it: the host finds across the whole chat, held here or not,
 * newest first, counts the places a bounded step at a time, and stepping past the places it sent asks for more.
 */
export class ChatFind {
  private listeners = new Set<() => void>();
  private text = '';
  private matchCase = false;
  private wholeWord = false;
  /** The number of the last ask, so an answer to an older query is let go. */
  private asked = 0;
  private hits: FindHit[] = [];
  /** Where the host goes on from for older places; null once it has sent them all. */
  private next: FindAt | null = null;
  /** The place among `hits` the reader is on. */
  private at: number | null = null;
  /** Stepping older went past `hits`, and waits for more. */
  private stepping = false;
  /** The ask out only counts, so its places are not wanted. */
  private counting = false;
  private counted = 0;
  private complete = false;
  private typing: ReturnType<typeof setTimeout> | undefined;
  private state: FindState;

  constructor(
    private readonly session: ChatSession,
    /** Brings a place into view once its row is held. */
    private readonly show: (hit: FindHit) => void,
  ) {
    this.state = this.build();
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  snapshot = () => this.state;

  setText = (text: string) => {
    this.text = text;
    clearTimeout(this.typing);
    this.typing = setTimeout(this.search, TYPING_MS);
    this.emit();
  };

  flipCase = () => {
    this.matchCase = !this.matchCase;
    this.search();
  };

  flipWord = () => {
    this.wholeWord = !this.wholeWord;
    this.search();
  };

  /** Moves to the next place found, older when `older`, across the whole chat. */
  step = (older: boolean) => {
    if (!this.hits.length) {
      if (older && this.next !== null) this.stepping = true;
      return;
    }
    const last = this.hits.length - 1;
    const at = this.at;
    const to =
      at === null ? 0 : older ? (at < last ? at + 1 : this.next === null ? 0 : null) : at > 0 ? at - 1 : this.next === null ? last : null;
    if (to === null) {
      if (older) {
        this.stepping = true;
        this.ask(this.next, false);
      }
      return;
    }
    this.at = to;
    this.goTo(to);
    this.emit();
  };

  close = () => {
    clearTimeout(this.typing);
    this.asked += 1;
  };

  private search = () => {
    clearTimeout(this.typing);
    this.hits = [];
    this.next = null;
    this.at = null;
    this.stepping = false;
    this.counted = 0;
    this.complete = false;
    this.asked += 1;
    if (this.text) this.ask(null, false);
    this.emit();
  };

  private ask(from: FindAt | null, countOnly: boolean) {
    const asked = (this.asked += 1);
    this.counting = countOnly;
    this.session
      .find({
        query: this.text,
        matchCase: this.matchCase,
        wholeWord: this.wholeWord,
        direction: 'older',
        limit: HITS,
        ...(from === null ? {} : { from, countOnly }),
      })
      .then(
        (found) => this.found(asked, found),
        () => {
          if (asked !== this.asked) return;
          this.complete = true;
          this.emit();
        },
      );
  }

  /** Takes what the host found, asks it to go on counting until the count is whole or large, and goes on a step that waited for more. */
  private found(asked: number, found: FindAnswer) {
    if (asked !== this.asked) return;
    if (!this.counting) {
      this.hits = [...this.hits, ...found.hits];
      this.next = found.next ?? null;
    }
    this.counting = false;
    this.counted = Math.max(this.counted, found.counted.matches);
    this.complete = found.counted.complete;
    // A long chat's first slice may hold no place at all, so places are asked for until one comes.
    if (!this.hits.length && this.next !== null) {
      this.ask(this.next, false);
      return this.emit();
    }
    const stepping = this.stepping;
    this.stepping = false;
    if (this.at === null && this.hits.length) {
      this.at = 0;
      this.goTo(0);
    } else if (stepping) {
      this.step(true);
    }
    const countOn = !this.complete && this.counted < MOST_COUNTED && found.next != null;
    if (countOn && asked === this.asked) this.ask(found.next, true);
    this.emit();
  }

  private goTo(at: number) {
    const hit = this.hits[at];
    void this.session.reach(hit.row).then((held) => {
      if (held && this.hits[this.at ?? -1] === hit) this.show(hit);
    });
  }

  private label(): string {
    if (!this.text) return '';
    if (this.counted === 0) return this.complete && !this.hits.length ? 'No results' : '…';
    if (this.counted >= MOST_COUNTED) return `${(this.at ?? 0) + 1}/999+`;
    const place = this.at === null ? this.counted : this.counted - this.at;
    return `${place}/${this.counted}${this.complete ? '' : '…'}`;
  }

  private markedFrom: FindHit[] | undefined;
  private byPart: Finding['byPart'] = new Map();

  private build(): FindState {
    if (this.markedFrom !== this.hits) {
      this.markedFrom = this.hits;
      const byPart = new Map<string, Range[]>();
      for (const hit of this.hits) {
        const ranges = byPart.get(hit.part);
        if (ranges) ranges.push(hit.range);
        else byPart.set(hit.part, [hit.range]);
      }
      for (const ranges of byPart.values()) ranges.sort((a, b) => a[0] - b[0]);
      this.byPart = byPart;
    }
    const current = this.at === null ? null : this.hits[this.at];
    return {
      text: this.text,
      matchCase: this.matchCase,
      wholeWord: this.wholeWord,
      label: this.label(),
      finding: this.hits.length ? { byPart: this.byPart, current: current ? { part: current.part, range: current.range } : null } : null,
    };
  }

  private emit() {
    this.state = this.build();
    this.listeners.forEach((listener) => listener());
  }
}
