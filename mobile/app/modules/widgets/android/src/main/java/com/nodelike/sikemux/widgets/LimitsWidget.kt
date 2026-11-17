package com.nodelike.sikemux.widgets

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.view.View
import android.widget.RemoteViews
import org.json.JSONArray
import org.json.JSONObject

data class Limit(val label: String, val used: Double, val resets: String)

/** How much of each agent account's plan is used, as the app last read it from the hosts. */
class LimitsWidget : AppWidgetProvider() {
  override fun onUpdate(context: Context, manager: AppWidgetManager, ids: IntArray) {
    val views = draw(context, read(context))
    ids.forEach { manager.updateAppWidget(it, views) }
  }

  companion object {
    private const val PREFS = "sikemux.widgets"
    private const val LIMITS = "limits"
    private val ROWS = listOf(R.id.limit_0, R.id.limit_1, R.id.limit_2, R.id.limit_3)
    private val LABELS = listOf(R.id.limit_0_label, R.id.limit_1_label, R.id.limit_2_label, R.id.limit_3_label)
    private val BARS = listOf(R.id.limit_0_bar, R.id.limit_1_bar, R.id.limit_2_bar, R.id.limit_3_bar)
    private val WARN_BARS = listOf(R.id.limit_0_warn, R.id.limit_1_warn, R.id.limit_2_warn, R.id.limit_3_warn)
    private val USED = listOf(R.id.limit_0_used, R.id.limit_1_used, R.id.limit_2_used, R.id.limit_3_used)
    private val RESETS = listOf(R.id.limit_0_resets, R.id.limit_1_resets, R.id.limit_2_resets, R.id.limit_3_resets)
    /** Past this share the bar turns amber, as the Mac's ruler does. */
    private const val WARN_AT = 80.0

    fun save(context: Context, limits: List<Limit>) {
      val json = JSONArray(limits.map { JSONObject().put("label", it.label).put("used", it.used).put("resets", it.resets) })
      context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(LIMITS, json.toString()).apply()
      val manager = AppWidgetManager.getInstance(context)
      val ids = manager.getAppWidgetIds(ComponentName(context, LimitsWidget::class.java))
      if (ids.isNotEmpty()) manager.updateAppWidget(ids, draw(context, limits))
    }

    private fun read(context: Context): List<Limit> {
      val text = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(LIMITS, null) ?: return emptyList()
      val json = JSONArray(text)
      return (0 until json.length()).map { json.getJSONObject(it) }
        .map { Limit(it.getString("label"), it.getDouble("used"), it.getString("resets")) }
    }

    private fun draw(context: Context, limits: List<Limit>): RemoteViews {
      val views = RemoteViews(context.packageName, R.layout.sikemux_limits_widget)
      views.setViewVisibility(R.id.limits_empty, if (limits.isEmpty()) View.VISIBLE else View.GONE)
      ROWS.indices.forEach { index ->
        val limit = limits.getOrNull(index)
        views.setViewVisibility(ROWS[index], if (limit == null) View.GONE else View.VISIBLE)
        if (limit == null) return@forEach
        val warn = limit.used >= WARN_AT
        val share = limit.used.coerceIn(0.0, 100.0).toInt()
        views.setTextViewText(LABELS[index], limit.label)
        views.setViewVisibility(BARS[index], if (warn) View.GONE else View.VISIBLE)
        views.setViewVisibility(WARN_BARS[index], if (warn) View.VISIBLE else View.GONE)
        views.setProgressBar(if (warn) WARN_BARS[index] else BARS[index], 100, share, false)
        views.setTextViewText(USED[index], "${limit.used.toInt()}%")
        views.setTextViewText(RESETS[index], limit.resets)
      }
      context.packageManager.getLaunchIntentForPackage(context.packageName)?.let { launch ->
        val open = PendingIntent.getActivity(context, 0, launch, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        views.setOnClickPendingIntent(R.id.limits_root, open)
      }
      return views
    }
  }
}
