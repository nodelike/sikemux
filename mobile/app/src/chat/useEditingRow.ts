import { useEffect, useMemo } from 'react';
import { Keyboard } from 'react-native';

import type { ChatMessage } from '@shared/wire.generated';

import type { ChatSession } from './session';

/** What the row being written again sends and cancels, and keeping it just above the keyboard once that is up. */
export function useEditingRow(
  editing: string | undefined,
  setEditing: (id: string | undefined) => void,
  session: ChatSession,
  scroll: { reveal: (index: number, at: number) => void; toEnd: () => void },
  rows: ChatMessage[],
) {
  const editingIndex = editing === undefined ? -1 : rows.findIndex((row) => row.id === editing);
  useEffect(() => {
    if (editingIndex < 0) return;
    const shown = Keyboard.addListener('keyboardDidShow', () => scroll.reveal(editingIndex, 1));
    return () => shown.remove();
  }, [editingIndex, scroll]);
  return useMemo(
    () =>
      editing === undefined
        ? undefined
        : {
            onCancel: () => setEditing(undefined),
            onSend: (text: string) => {
              session.edit(editing, text);
              setEditing(undefined);
              scroll.toEnd();
            },
          },
    [editing, setEditing, session, scroll],
  );
}
