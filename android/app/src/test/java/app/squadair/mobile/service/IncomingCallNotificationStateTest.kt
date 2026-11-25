package app.squadair.mobile.service

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class IncomingCallNotificationStateTest {
    @Test
    fun foregroundCall_doesNotPostNotification() {
        val coord = CallNotificationCoordinator()
        val res = coord.onIncomingCall("squad://room1#key", isUiVisible = true)
        assertEquals(CallNotificationAction.NONE, res.action)
        assertTrue(res.shouldUpdateIncomingCallState)
        assertNull(coord.postedInviteUri)
    }

    @Test
    fun backgroundCall_postsOnceAndDeduplicatesSameInvite() {
        val coord = CallNotificationCoordinator()
        val first = coord.onIncomingCall("squad://room1#key", isUiVisible = false)
        assertEquals(CallNotificationAction.POST_NOTIFICATION, first.action)
        assertTrue(first.shouldUpdateIncomingCallState)
        assertEquals("squad://room1#key", coord.postedInviteUri)

        val dup = coord.onIncomingCall("squad://room1#key", isUiVisible = false)
        assertEquals(CallNotificationAction.NONE, dup.action)
        assertFalse(dup.shouldUpdateIncomingCallState)
    }

    @Test
    fun returningToForeground_cancelsNotificationButKeepsIncomingCall() {
        val coord = CallNotificationCoordinator()
        coord.onIncomingCall("squad://room1#key", isUiVisible = false)

        val fgAction = coord.onAppForegrounded()
        assertEquals(CallNotificationAction.CANCEL_NOTIFICATION, fgAction)
        assertEquals("squad://room1#key", coord.currentInviteUri)
        assertNull(coord.postedInviteUri)
    }

    @Test
    fun acceptDeclineAndJoinedRoom_cancelNotificationAndClearInvite() {
        val coord = CallNotificationCoordinator()
        coord.onIncomingCall("squad://room1#key", isUiVisible = false)
        assertEquals(CallNotificationAction.CANCEL_NOTIFICATION, coord.onAccept())
        assertNull(coord.currentInviteUri)

        coord.onIncomingCall("squad://room2#key", isUiVisible = false)
        assertEquals(CallNotificationAction.CANCEL_NOTIFICATION, coord.onDecline())
        assertNull(coord.currentInviteUri)

        coord.onIncomingCall("squad://room3#key", isUiVisible = false)
        assertEquals(CallNotificationAction.CANCEL_NOTIFICATION, coord.onJoinedRoom())
        assertNull(coord.currentInviteUri)
    }

    @Test
    fun timeout_clearsMatchingInviteAndIgnoresStaleTimeout() {
        val coord = CallNotificationCoordinator()
        coord.onIncomingCall("squad://old#key", isUiVisible = false)
        coord.onIncomingCall("squad://new#key", isUiVisible = false)

        val stale = coord.onTimeout("squad://old#key")
        assertEquals(CallNotificationAction.NONE, stale.action)
        assertFalse(stale.clearedIncomingCall)
        assertEquals("squad://new#key", coord.currentInviteUri)

        val active = coord.onTimeout("squad://new#key")
        assertEquals(CallNotificationAction.CANCEL_NOTIFICATION, active.action)
        assertTrue(active.clearedIncomingCall)
        assertNull(coord.currentInviteUri)
    }
}
