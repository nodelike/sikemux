import type {
  ApnsEnvironment,
  PushKind,
  PushPlatform,
} from "@sikemux/protocol";

/** One notification for one phone, as a platform's push service receives it. */
export interface PushMessage {
  token: string;
  kind: PushKind;
  collapseId: string;
  blob: string;
  /** How long the platform may keep trying to reach the phone. */
  ttlSeconds: number;
  /** Which of Apple's push servers issued an iOS token. */
  apnsEnvironment?: ApnsEnvironment;
  /** Sent the time the host pushed it, so a Live Activity keeps the newest of two that cross. */
  sentAt?: number;
}

/** What a push service did with one attempt. */
export type Delivery =
  | { outcome: "sent" }
  /** The token will never work again: the app was uninstalled, or the token belongs elsewhere. */
  | { outcome: "dead"; reason: string }
  /** Worth trying again, no sooner than afterMs when the service said so. */
  | { outcome: "retry"; reason: string; afterMs?: number }
  | { outcome: "failed"; reason: string };

export interface PushProvider {
  send(message: PushMessage): Promise<Delivery>;
}

/** The push services this server can reach. A platform without one answers not_set_up. */
export type PushProviders = Partial<Record<PushPlatform, PushProvider>>;
