package com.nodelike.sikemux.notify

import java.io.File
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class EnvelopeTest {
  private val vector = JSONObject(vectorFile().readText())
  private val host = vector.getString("hostKey")
  private val phone = vector.getString("phoneKey")
  private val keyId = vector.getLong("keyIdDecimal")
  private val key = unhex(vector.getString("key"))

  @Test
  fun opensTheVectorTheHostSealed() {
    val opened = Envelope.open(vector.getString("blob"), phone, listOf(HostKey(host, keyId, key)))
    assertTrue(opened is Opened.Read)
    opened as Opened.Read
    assertEquals(host, opened.host)
    assertEquals(vector.getString("plaintext"), opened.plaintext)
    assertEquals(vector.getString("aad"), String(Envelope.aad(host, phone, keyId)))
  }

  @Test
  fun readsTheCardInside() {
    val opened = Envelope.open(vector.getString("blob"), phone, listOf(HostKey(host, keyId, key))) as Opened.Read
    val card = parseCard(opened.plaintext, opened.host)
    assertNotNull(card)
    val expected = vector.getJSONObject("notification")
    assertEquals(expected.getString("collapseId"), card!!.collapseId)
    assertEquals(expected.getString("url"), card.url)
    assertEquals(expected.getString("detail"), card.detail)
    assertEquals(expected.getString("allowOptionId"), card.allowOptionId)
    assertTrue(card.answerable)
    assertEquals(null, parseCard(opened.plaintext, phone))
  }

  @Test
  fun aPushForAnotherPhoneOrHostDoesNotOpen() {
    val swapped = Envelope.open(vector.getString("blob"), host, listOf(HostKey(phone, keyId, key)))
    assertTrue(swapped is Opened.Unreadable)
    val wrongKey = Envelope.open(vector.getString("blob"), phone, listOf(HostKey(host, keyId, ByteArray(32))))
    assertTrue(wrongKey is Opened.Unreadable)
  }

  @Test
  fun aPushUnderAKeyThePhoneLacksOrANewerVersionIsNotRead() {
    assertSame(Opened.NoKey, Envelope.open(vector.getString("blob"), phone, listOf(HostKey(host, keyId + 1, key))))
    val newer = java.util.Base64.getDecoder().decode(vector.getString("blob")).also { it[0] = 2 }
    assertSame(Opened.Unknown, Envelope.open(java.util.Base64.getEncoder().encodeToString(newer), phone, listOf(HostKey(host, keyId, key))))
    assertSame(Opened.Unknown, Envelope.open("not base64!", phone, emptyList()))
  }

  private fun vectorFile(): File {
    var dir: File? = File("").absoluteFile
    while (dir != null) {
      val candidate = File(dir, "server/protocol/vectors/push.json")
      if (candidate.exists()) return candidate
      dir = dir.parentFile
    }
    error("server/protocol/vectors/push.json is not above ${File("").absolutePath}")
  }

  private fun unhex(text: String) = ByteArray(text.length / 2) { text.substring(it * 2, it * 2 + 2).toInt(16).toByte() }
}
