export enum CryptoDigestAlgorithm {
  SHA256 = 'SHA-256',
}

export const randomUUID = (): string => crypto.randomUUID();

export const getRandomBytes = (count: number): Uint8Array => crypto.getRandomValues(new Uint8Array(count));

export async function digestStringAsync(algorithm: CryptoDigestAlgorithm, text: string): Promise<string> {
  const digest = await crypto.subtle.digest(algorithm, new TextEncoder().encode(text));
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('');
}
