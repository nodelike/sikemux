import { requireOptionalNativeModule } from 'expo';

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
};

/** The phone's half of notifications: the keys hosts seal cards with, and the cards showing. */
export const notifier = requireOptionalNativeModule<NotifyModule>('SikemuxNotify');
