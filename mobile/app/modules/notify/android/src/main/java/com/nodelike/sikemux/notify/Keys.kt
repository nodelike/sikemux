package com.nodelike.sikemux.notify

import android.content.Context
import android.content.SharedPreferences
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.security.KeyStore
import java.util.Base64
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * The notification key this phone gave each host, kept apart from the phone's own key: it can only
 * read notifications, so the messaging service may open it whenever a push arrives, locked or not.
 * Each is stored wrapped by a key that never leaves the Android Keystore.
 */
class Keys(context: Context) {
  private val prefs: SharedPreferences = context.getSharedPreferences(FILE, Context.MODE_PRIVATE)

  var phone: String?
    get() = prefs.getString(PHONE, null)
    set(value) {
      prefs.edit().putString(PHONE, value).apply()
    }

  fun put(host: String, keyId: Long, key: ByteArray) {
    prefs.edit()
      .putString(HOST + host, entry(keyId, wrap(key)))
      .remove(UNREADABLE + host)
      .commit()
  }

  /** The host's key, unless a push showed the host no longer seals with it. */
  fun get(host: String): HostKey? {
    if (prefs.getBoolean(UNREADABLE + host, false)) return null
    return read(host, prefs.getString(HOST + host, null) ?: return null)
  }

  fun all(): List<HostKey> =
    prefs.all.mapNotNull { (name, value) ->
      if (name.startsWith(HOST) && value is String) read(name.removePrefix(HOST), value) else null
    }

  fun unreadable(host: String) {
    prefs.edit().putBoolean(UNREADABLE + host, true).apply()
  }

  fun remove(host: String) {
    prefs.edit().remove(HOST + host).remove(UNREADABLE + host).commit()
  }

  fun clear() {
    prefs.edit().clear().commit()
  }

  private fun read(host: String, stored: String): HostKey? {
    val (keyId, wrapped) = parseEntry(stored) ?: return null
    return try {
      HostKey(host, keyId, unwrap(wrapped))
    } catch (_: Exception) {
      null
    }
  }

  private fun wrapping(): SecretKey = synchronized(LOCK) {
    val store = KeyStore.getInstance(KEYSTORE).apply { load(null) }
    (store.getKey(ALIAS, null) as? SecretKey)?.let { return it }
    val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
    generator.init(
      KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
        .setKeySize(256)
        .build(),
    )
    generator.generateKey()
  }

  private fun wrap(key: ByteArray): String {
    val cipher = Cipher.getInstance("AES/GCM/NoPadding")
    cipher.init(Cipher.ENCRYPT_MODE, wrapping())
    return Base64.getEncoder().encodeToString(cipher.iv + cipher.doFinal(key))
  }

  private fun unwrap(wrapped: String): ByteArray {
    val bytes = Base64.getDecoder().decode(wrapped)
    val cipher = Cipher.getInstance("AES/GCM/NoPadding")
    cipher.init(Cipher.DECRYPT_MODE, wrapping(), GCMParameterSpec(128, bytes, 0, 12))
    return cipher.doFinal(bytes, 12, bytes.size - 12)
  }

  companion object {
    private val LOCK = Any()
    private const val FILE = "sikemux-notify"
    private const val KEYSTORE = "AndroidKeyStore"
    private const val ALIAS = "sikemux-notify"
    private const val PHONE = "phone"
    private const val HOST = "host:"
    private const val UNREADABLE = "unreadable:"

    fun entry(keyId: Long, wrapped: String): String = "$keyId:$wrapped"

    /** The key id and the wrapped key stored for a host, or null for anything [entry] did not write. */
    fun parseEntry(stored: String): Pair<Long, String>? {
      val (keyId, wrapped) = stored.split(':', limit = 2).takeIf { it.size == 2 } ?: return null
      return Pair(keyId.toLongOrNull() ?: return null, wrapped.takeIf { it.isNotEmpty() } ?: return null)
    }
  }
}
