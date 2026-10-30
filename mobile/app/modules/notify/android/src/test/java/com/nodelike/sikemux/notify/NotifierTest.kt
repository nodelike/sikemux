package com.nodelike.sikemux.notify

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class NotifierTest {
  @Test
  fun aCardOnAChannelThisAppDoesNotMakeGoesToNeedsYou() {
    assertEquals(Notifier.FINISHED, Notifier.channel(Notifier.FINISHED))
    assertEquals(Notifier.PROBLEMS, Notifier.channel(Notifier.PROBLEMS))
    assertEquals(Notifier.NEEDS_YOU, Notifier.channel("reminders"))
    assertEquals(Notifier.NEEDS_YOU, Notifier.channel(null))
  }

  @Test
  fun eachCardsAnswersAreTheirOwnIntents() {
    val allow = Notifier.answerData("card-1", "allow")
    assertNotEquals(allow, Notifier.answerData("card-1", "reject"))
    assertNotEquals(allow, Notifier.answerData("card-2", "allow"))
    assertNotEquals(Notifier.answerData("a/b", "c"), Notifier.answerData("a", "b/c"))
    assertEquals(allow, Notifier.answerData("card-1", "allow"))
  }

  @Test
  fun aCardLeftSendingOutlastsTheAnswersTimeLimit() {
    assertTrue(Notifier.SENDING_MS > AnswerService.TIMEOUT_MS)
  }

  @Test
  fun aCardPastItsLifeHasNoTimeLeft() {
    val card = card(expiresAt = 10_000)
    assertEquals(4_000L, card.timeLeft(6_000))
    assertNull(card.timeLeft(10_000))
    assertNull(card.timeLeft(12_000))
  }

  private fun card(expiresAt: Long) = NotificationCard(
    v = CARD_VERSION,
    kind = NotificationKind.PERMISSION,
    channel = NotificationChannel.NEEDS_YOU,
    category = NotificationCategory.PERMISSION,
    collapseId = "card-1",
    thread = "thread",
    hostKey = "host",
    hostName = "Desk",
    agentId = "agent",
    provider = "claude",
    chatTitle = null,
    title = "Run a command?",
    body = "ls",
    detail = null,
    url = "sikemux://device/host",
    requestId = "request",
    allowOptionId = "allow",
    rejectOptionId = "reject",
    at = 0,
    expiresAt = expiresAt,
  )}
