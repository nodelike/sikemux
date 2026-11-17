package com.nodelike.sikemux.notify

import android.content.Context
import expo.modules.kotlin.exception.Exceptions
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition

class NotifyModule : Module() {
  private val context: Context
    get() = appContext.reactContext?.applicationContext ?: throw Exceptions.ReactContextLost()

  override fun definition() = ModuleDefinition {
    Name("SikemuxNotify")

    OnCreate {
      appContext.reactContext?.applicationContext?.let { Notifier(it).ensureChannels() }
    }

    Function("setPhone") { phone: String ->
      Keys(context).phone = phone
    }

    Function("key") { host: String ->
      Keys(context).get(host)?.let { mapOf("keyId" to it.keyId.toDouble(), "key" to hex(it.key)) }
    }

    Function("setKey") { host: String, keyId: Double, key: String ->
      Keys(context).put(host, keyId.toLong(), unhex(key))
    }

    Function("removeKey") { host: String ->
      Keys(context).remove(host)
    }

    Function("removeAll") {
      Keys(context).clear()
      Notifier(context).cancelAll()
    }

    AsyncFunction("shown") {
      Notifier(context).shown()
    }

    Function("dismiss") { tag: String ->
      Notifier(context).cancel(tag)
    }

    Function("settle") { tag: String, outcome: String ->
      AnswerService.settle(context, tag, outcome)
    }

    Function("showAgents") { title: String, text: String?, url: String? ->
      Notifier(context).showAgents(title, text, url)
    }

    Function("cancelAgents") {
      Notifier(context).cancelAgents()
    }
  }

  private fun hex(bytes: ByteArray) = bytes.joinToString("") { "%02x".format(it) }

  private fun unhex(text: String): ByteArray {
    require(text.length == 64 && text.all { it in '0'..'9' || it in 'a'..'f' }) { "a notification key is 64 lowercase hex characters" }
    return ByteArray(32) { text.substring(it * 2, it * 2 + 2).toInt(16).toByte() }
  }
}
