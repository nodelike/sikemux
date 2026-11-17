package com.nodelike.sikemux.widgets

import android.content.Context
import expo.modules.kotlin.exception.Exceptions
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import expo.modules.kotlin.records.Field
import expo.modules.kotlin.records.Record

class LimitLine : Record {
  @Field val label: String = ""
  @Field val used: Double = 0.0
  @Field val resets: String = ""
}

class WidgetsModule : Module() {
  private val context: Context
    get() = appContext.reactContext?.applicationContext ?: throw Exceptions.ReactContextLost()

  override fun definition() = ModuleDefinition {
    Name("SikemuxWidgets")

    Function("setLimits") { lines: List<LimitLine> ->
      LimitsWidget.save(context, lines.map { Limit(it.label, it.used, it.resets) })
    }
  }
}
