import { aesDecryptAsync, aesEncryptAsync, AESEncryptionKey, AESSealedData, CryptoDigestAlgorithm, digest } from 'expo-crypto';

import { fromBase64Url, NONCE_BYTES, shareAad, toBase64Url, viewerProofText, type ShareCipher } from '@shared/share/cipher';

const SHA256_BLOCK = 64;

const sha256 = async (...parts: Uint8Array[]) => {
  const joined = new Uint8Array(parts.reduce((total, part) => total + part.length, 0));
  let at = 0;
  for (const part of parts) {
    joined.set(part, at);
    at += part.length;
  }
  return new Uint8Array(await digest(CryptoDigestAlgorithm.SHA256, joined));
};

/** HMAC-SHA256 from expo-crypto's SHA-256, which has no HMAC of its own; `key` is at most one block. */
async function hmacSha256(key: Uint8Array, message: Uint8Array): Promise<Uint8Array> {
  const pad = (byte: number) => Uint8Array.from({ length: SHA256_BLOCK }, (_, index) => (key[index] ?? 0) ^ byte);
  return sha256(pad(0x5c), await sha256(pad(0x36), message));
}
import type { ShareLink } from '@shared/share/link';

/**
 * AES-256-GCM in the phone's native crypto, since its JavaScript has no WebCrypto. A share's
 * sealed text is the nonce, the ciphertext, then the tag: the layout expo-crypto calls combined.
 */
export function phoneCipher(link: ShareLink): ShareCipher {
  const key = AESEncryptionKey.import(fromBase64Url(link.key));
  const aad = new TextEncoder().encode(shareAad(link.id));
  return {
    async open(sealed) {
      try {
        const data = AESSealedData.fromCombined(fromBase64Url(sealed), { ivLength: NONCE_BYTES, tagLength: 16 });
        const plain = await aesDecryptAsync(data, await key, { additionalData: aad });
        return new TextDecoder().decode(plain);
      } catch {
        return null;
      }
    },
    async seal(text) {
      const data = await aesEncryptAsync(new TextEncoder().encode(text), await key, { additionalData: aad });
      return toBase64Url(await data.combined());
    },
    async viewerProof() {
      return toBase64Url(await hmacSha256(fromBase64Url(link.key), new TextEncoder().encode(viewerProofText(link.id))));
    },
  };
}
