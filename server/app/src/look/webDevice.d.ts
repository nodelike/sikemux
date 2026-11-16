declare module "@web-device" {
  export default function init(): Promise<unknown>;
  export function newKey(): Uint8Array;
  export function publicKey(key: Uint8Array): string;
  export function signRegistration(
    key: Uint8Array,
    nonce: string,
    userId: string,
  ): string;
  export function readLook(
    key: Uint8Array,
    relays: string[],
    host: string,
    ticket: string,
  ): Promise<string>;
}
