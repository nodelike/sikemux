import { requireOptionalNativeModule } from 'expo';
import type { ApnsEnvironment } from '@sikemux/protocol';

export type ShownCard = { tag: string; host: string; agent?: string; kind?: string; request?: string };

export type AnswerOutcome = 'answered' | 'rejected' | 'gone' | 'failed';

type NotifyModule = {
  setPhone(phone: string): void;
  key(host: string): { keyId: number; key: string } | null;
  setKey(host: string, keyId: number, key: string): void;
  removeKey(host: string): void;
  removeAll(): void;
  shown(): Promise<ShownCard[]>;
  dismiss(tag: string): void;
  settle(tag: string, outcome: AnswerOutcome): void;
  /** iOS only: which of Apple's push servers issued this build's token. */
  apnsEnvironment?(): ApnsEnvironment;
};

/** The phone's half of notifications: the keys hosts seal cards with, and the cards showing. */
export const notifier = requireOptionalNativeModule<NotifyModule>('SikemuxNotify');
