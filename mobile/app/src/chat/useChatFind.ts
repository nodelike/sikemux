import { useEffect, useRef, useState, useSyncExternalStore } from 'react';

import { shownParts } from '@shared/chat/transcript';
import type { ChatMessage } from '@shared/chat/types';
import type { FindHit } from '@shared/wire.generated';

import { ChatFind } from './chatFind';
import { rowFold } from './folds';
import type { ChatSession } from './session';

const noSubscribe = () => () => {};

/** The find bar's state: the search that is open, what it found, and the row brought into view for it. */
export function useChatFind(session: ChatSession, rows: ChatMessage[], workOpen: (id: string) => boolean, onOpen: () => void) {
  const [find, setFind] = useState<ChatFind | null>(null);
  const finding = useSyncExternalStore(find?.subscribe ?? noSubscribe, () => find?.snapshot().finding ?? null);
  // A place found is brought into view once the rows it unfolded or paged in are drawn.
  const [shown, setShown] = useState<{ row: string }>();
  const showHit = (hit: FindHit) => {
    const row = session.snapshot().view.messages.find((message) => message.id === hit.row);
    if (!row) return;
    const { from } = rowFold(row, workOpen);
    const tucked = !shownParts(row.parts, from).some((held) => held.id === hit.part);
    if (row.fold && tucked) session.folds.set(`work/${row.fold.id}`, true);
    setShown({ row: hit.row });
  };
  const showing = useRef(showHit);
  useEffect(() => {
    showing.current = showHit;
  });
  const shownIndex = shown ? rows.findIndex((row) => row.id === shown.row) : -1;
  const openFind = () => {
    onOpen();
    setFind(new ChatFind(session, (hit) => showing.current(hit)));
  };
  const closeFind = () => {
    find?.close();
    setFind(null);
    setShown(undefined);
  };
  return { find, finding, shown, shownIndex, openFind, closeFind };
}

/** Only a new place moves the transcript, not the rows growing under it. */
export function useRevealFound(
  scroll: { reveal: (index: number, at: number) => void },
  shown: { row: string } | undefined,
  shownIndex: number,
) {
  const revealed = useRef(shown);
  useEffect(() => {
    if (shownIndex < 0 || revealed.current === shown) return;
    revealed.current = shown;
    scroll.reveal(shownIndex, 0.3);
  }, [shown, shownIndex, scroll]);
}
