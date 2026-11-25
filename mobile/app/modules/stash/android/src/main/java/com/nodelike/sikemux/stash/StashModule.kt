package com.nodelike.sikemux.stash

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.OpenableColumns
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import java.io.File
import java.io.IOException
import java.util.UUID

/** The most of one shared file kept; the stash takes no more. */
private const val MOST_BYTES = 25L * 1024 * 1024

/**
 * What another app shared to Sikemux through the share menu, read from the intent that opened it:
 * its text and subject, and each file copied into the cache, since the right to read it ends with
 * the intent.
 */
class StashModule : Module() {
  private var pending: Intent? = null

  override fun definition() = ModuleDefinition {
    Name("SikemuxStash")
    Events("onShare")

    OnNewIntent { intent ->
      if (isShare(intent)) {
        pending = intent
        sendEvent("onShare", emptyMap<String, Any>())
      }
    }

    AsyncFunction("take") {
      val activity = appContext.currentActivity
      val intent = pending ?: activity?.intent?.takeIf(::isShare) ?: return@AsyncFunction null
      pending = null
      activity?.intent = Intent(Intent.ACTION_MAIN)
      read(intent)
    }

    Function("leave") {
      appContext.currentActivity?.moveTaskToBack(true)
    }
  }

  private fun isShare(intent: Intent) = intent.action == Intent.ACTION_SEND || intent.action == Intent.ACTION_SEND_MULTIPLE

  private fun read(intent: Intent): Map<String, Any?> {
    val files = streams(intent).mapNotNull { copy(it, intent.type) }
    return mapOf(
      "text" to intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString(),
      "subject" to intent.getStringExtra(Intent.EXTRA_SUBJECT),
      "files" to files,
    )
  }

  @Suppress("DEPRECATION")
  private fun streams(intent: Intent): List<Uri> {
    val found = mutableListOf<Uri>()
    if (intent.action == Intent.ACTION_SEND_MULTIPLE) {
      val list = if (Build.VERSION.SDK_INT >= 33) intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
      else intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM)
      list?.let { found.addAll(it) }
    } else {
      val one = if (Build.VERSION.SDK_INT >= 33) intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
      else intent.getParcelableExtra(Intent.EXTRA_STREAM)
      one?.let { found.add(it) }
    }
    if (found.isEmpty()) intent.clipData?.let { clip -> (0 until clip.itemCount).mapNotNullTo(found) { clip.getItemAt(it).uri } }
    return found.distinct()
  }

  /** The file at `uri` in the cache, or one too large to stash with only its name and size. */
  private fun copy(uri: Uri, shared: String?): Map<String, Any?>? {
    val context = appContext.reactContext ?: return null
    val resolver = context.contentResolver
    var name = uri.lastPathSegment?.substringAfterLast('/') ?: "file"
    var size = -1L
    try {
      resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { cursor ->
        if (cursor.moveToFirst()) {
          cursor.getString(0)?.let { name = it }
          if (!cursor.isNull(1)) size = cursor.getLong(1)
        }
      }
    } catch (_: SecurityException) {
      return null
    } catch (_: RuntimeException) {
      return null
    }
    val mime = try {
      resolver.getType(uri)
    } catch (_: SecurityException) {
      return null
    } ?: shared?.takeUnless { it.endsWith("/*") } ?: "application/octet-stream"
    if (size > MOST_BYTES) return mapOf("name" to name, "mime" to mime, "size" to size, "uri" to null)
    val safeName = name
      .substringAfterLast('/')
      .substringAfterLast('\\')
      .replace('\u0000', '_')
      .trim()
      .takeIf { it.isNotEmpty() && it != "." && it != ".." }
      ?: "file"
    val folder = File(context.cacheDir, "shared/${UUID.randomUUID()}").apply { mkdirs() }
    val target = File(folder, safeName)
    try {
      if (!target.canonicalPath.startsWith(folder.canonicalPath + File.separator)) {
        folder.deleteRecursively()
        return null
      }
      var copied = 0L
      val input = resolver.openInputStream(uri)
      if (input == null) {
        folder.deleteRecursively()
        return null
      }
      input.use { source ->
        target.outputStream().use { sink ->
          val buffer = ByteArray(64 * 1024)
          while (true) {
            val read = source.read(buffer)
            if (read < 0) break
            copied += read
            if (copied > MOST_BYTES) break
            sink.write(buffer, 0, read)
          }
        }
      }
      if (copied > MOST_BYTES) {
        folder.deleteRecursively()
        return mapOf("name" to name, "mime" to mime, "size" to copied, "uri" to null)
      }
      return mapOf("name" to name, "mime" to mime, "size" to copied, "uri" to Uri.fromFile(target).toString())
    } catch (_: SecurityException) {
      folder.deleteRecursively()
      return null
    } catch (_: IOException) {
      folder.deleteRecursively()
      return null
    }
  }
}
