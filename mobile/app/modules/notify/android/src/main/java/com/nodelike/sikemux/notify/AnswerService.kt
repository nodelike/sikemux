package com.nodelike.sikemux.notify

import android.content.Context
import android.content.Intent
import com.facebook.react.HeadlessJsTaskService
import com.facebook.react.bridge.Arguments
import com.facebook.react.jstasks.HeadlessJsTaskConfig

/**
 * Runs the app's JavaScript without a screen to send an answer over the phone's own connection to the
 * host. The JavaScript says how it went through [settle]; an answer it never settled failed.
 */
class AnswerService : HeadlessJsTaskService() {
  override fun getTaskConfig(intent: Intent?): HeadlessJsTaskConfig? {
    val extras = intent?.extras ?: return null
    return HeadlessJsTaskConfig(TASK, Arguments.fromBundle(extras), TIMEOUT_MS, true)
  }

  override fun onDestroy() {
    super.onDestroy()
    synchronized(pending) { pending.keys.toList() }.forEach { settle(applicationContext, it, FAILED) }
  }

  private class Waiting(val hostName: String, val url: String?)

  companion object {
    const val TASK = "SikemuxAnswer"
    const val ANSWERED = "answered"
    const val SENT = "sent"
    const val REJECTED = "rejected"
    const val GONE = "gone"
    const val FAILED = "failed"
    const val TIMEOUT_MS = 25_000L
    private val pending = mutableMapOf<String, Waiting>()

    fun waiting(tag: String, hostName: String, url: String?) {
      synchronized(pending) { pending[tag] = Waiting(hostName, url) }
    }

    fun settle(context: Context, tag: String, outcome: String) {
      val waiting = synchronized(pending) { pending.remove(tag) } ?: return
      val notifier = Notifier(context)
      when (outcome) {
        ANSWERED -> notifier.showAnswer(tag, "Allowed on ${waiting.hostName}", null, waiting.url, Notifier.Answer.DONE)
        SENT -> notifier.showAnswer(tag, "Answered on ${waiting.hostName}", null, waiting.url, Notifier.Answer.DONE)
        REJECTED -> notifier.showAnswer(tag, "Rejected on ${waiting.hostName}", null, waiting.url, Notifier.Answer.DONE)
        GONE -> notifier.cancel(tag)
        else -> notifier.showAnswer(
          tag,
          "Couldn't reach ${waiting.hostName}",
          "Open Sikemux to answer",
          waiting.url,
          Notifier.Answer.FAILED,
        )
      }
    }
  }
}
