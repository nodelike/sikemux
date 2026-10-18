import { aesDecryptAsync, aesEncryptAsync, AESEncryptionKey, AESSealedData } from 'expo-crypto';

import { fromBase64Url, NONCE_BYTES, shareAad, toBase64Url, type ShareCipher } from '@mac/share/cipher';
import type { ShareLink } from '@mac/share/link';

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
  };
}
