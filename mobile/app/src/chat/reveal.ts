import { useEffect, useRef, useState } from 'react';

import { useStill } from '@/ui/motion';

/** The pace words come out at, in characters a millisecond, faster than anyone reads but never a jump. */
const PACE = 0.9;
/** How far behind the stream the words may fall before they speed up to keep within it. */
const MAX_BEHIND_MS = 1500;
/** How long a word takes to fade in. */
const FADE_MS = 180;
/** The shortest time between redraws of the text, so a 120 Hz screen redraws it no more than a 60 Hz one. */
const REDRAW_MS = 15;
/** How far past the pace a step may run to finish the word it stopped in. */
const WORD_REACH = 16;

export type Fading = { text: string; opacity: number }[];

type Step = { start: number; end: number; at: number };
type Shown = { id: string; length: number; carry: number; steps: Step[]; now: number };

function wordEnd(text: string, at: number): number {
  if (at >= text.length) return text.length;
  for (let end = at; end < Math.min(text.length, at + WORD_REACH); end += 1) if (/\s/.test(text[end])) return end;
  return at;
}

/** Words still fading in can only be drawn faint where they sit inside a line of plain text. */
const fadesInline = (text: string) => !/[\n`|]/.test(text);

/**
 * Streamed text as it should be drawn: its words come out at a steady pace however the network
 * delivers them, each fading in as it appears. `fading` is the end of `text` still coming in.
 */
export function useStreamedText(id: string, text: string, live: boolean): { text: string; fading: Fading } {
  const still = useStill();
  // Text already there when it is drawn shows whole; text still streaming comes out from its start.
  const fresh = (): Shown => ({ id, length: live ? 0 : text.length, carry: 0, steps: [], now: 0 });
  const [shown, setShown] = useState(fresh);
  if (shown.id !== id) setShown(fresh());
  const current = shown.id === id ? shown : fresh();
  const length = still ? text.length : Math.min(current.length, text.length);
  const moving = !still && (length < text.length || current.steps.length > 0);
  // Kept across new text arriving, which restarts the frame loop, so the pace counts real time.
  const last = useRef(0);

  useEffect(() => {
    if (!moving) {
      last.current = 0;
      return;
    }
    let frame = requestAnimationFrame(function tick(now) {
      if (last.current && now - last.current < REDRAW_MS) {
        frame = requestAnimationFrame(tick);
        return;
      }
      const since = last.current ? Math.min(now - last.current, 50) : 16;
      last.current = now;
      setShown((before) => {
        const from = Math.min(before.length, text.length);
        const behind = text.length - from;
        const carry = behind > 0 ? before.carry + Math.max(PACE, behind / MAX_BEHIND_MS) * since : 0;
        const end = behind > 0 ? wordEnd(text, from + Math.floor(carry)) : from;
        const steps = before.steps.filter((step) => now - step.at < FADE_MS && step.end <= end);
        if (end > from) steps.push({ start: from, end, at: now });
        return { id, length: end, carry: Math.max(0, carry - (end - from)), steps, now };
      });
      frame = requestAnimationFrame(tick);
    });
    return () => cancelAnimationFrame(frame);
  }, [moving, id, text]);

  const revealed = text.slice(0, length);
  const steps = current.steps.filter((step) => step.end <= length);
  if (!moving || !steps.length || !fadesInline(revealed.slice(steps[0].start))) return { text: revealed, fading: [] };
  const fading = steps.map((step) => ({
    text: revealed.slice(step.start, step.end),
    opacity: Math.min(1, (current.now - step.at) / FADE_MS),
  }));
  return { text: revealed, fading };
}
