package app.squadair.mobile.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AppLifecycleTrackerTest {
    @Test
    fun startsInBackgroundAndTracksSingleActivityForegroundAndStop() {
        val events = mutableListOf<Boolean>()
        val tracker = ForegroundStateTracker { events.add(it) }

        assertFalse(tracker.isInForeground)
        tracker.onActivityStarted(1)
        assertTrue(tracker.isInForeground)

        tracker.onActivityStopped(1, isChangingConfigurations = false)
        assertFalse(tracker.isInForeground)
        assertEquals(listOf(true, false), events)
    }

    @Test
    fun configurationChangeRecreationDoesNotDropForegroundState() {
        val events = mutableListOf<Boolean>()
        val tracker = ForegroundStateTracker { events.add(it) }

        tracker.onActivityStarted(1)
        tracker.onActivityStopped(1, isChangingConfigurations = true)
        assertTrue(tracker.isInForeground)

        tracker.onActivityStarted(2)
        assertTrue(tracker.isInForeground)
        assertEquals(listOf(true), events)

        tracker.onActivityStopped(2, isChangingConfigurations = false)
        assertFalse(tracker.isInForeground)
        assertEquals(listOf(true, false), events)
    }

    @Test
    fun multipleActivitiesRemainForegroundUntilAllStop() {
        val tracker = ForegroundStateTracker {}
        tracker.onActivityStarted(1)
        tracker.onActivityStarted(2)
        assertTrue(tracker.isInForeground)

        tracker.onActivityStopped(1, isChangingConfigurations = false)
        assertTrue(tracker.isInForeground)

        tracker.onActivityStopped(2, isChangingConfigurations = false)
        assertFalse(tracker.isInForeground)
    }

    @Test
    fun destroyedWithoutConfigChangeClearsPendingConfigChange() {
        val events = mutableListOf<Boolean>()
        val tracker = ForegroundStateTracker { events.add(it) }

        tracker.onActivityStarted(1)
        tracker.onActivityStopped(1, isChangingConfigurations = true)
        assertTrue(tracker.isInForeground)

        tracker.onActivityDestroyed(1, isChangingConfigurations = false)
        assertFalse(tracker.isInForeground)
        assertFalse(tracker.hasPendingConfigChanges())
        assertEquals(listOf(true, false), events)
    }

    @Test
    fun destroyedWithConfigChangeWithoutRestartExpiresCleanly() {
        val events = mutableListOf<Boolean>()
        val tracker = ForegroundStateTracker { events.add(it) }

        tracker.onActivityStarted(10)
        tracker.onActivityStopped(10, isChangingConfigurations = true)
        tracker.onActivityDestroyed(10, isChangingConfigurations = true)
        assertTrue(tracker.isInForeground)

        tracker.expirePendingConfigChange(10)
        assertFalse(tracker.isInForeground)
        assertFalse(tracker.hasPendingConfigChanges())
        assertEquals(listOf(true, false), events)
    }

    @Test
    fun destroyedWithoutPriorStopRemovesStartedActivity() {
        val tracker = ForegroundStateTracker {}
        tracker.onActivityStarted(7)
        assertTrue(tracker.isInForeground)

        tracker.onActivityDestroyed(7, isChangingConfigurations = false)
        assertFalse(tracker.isInForeground)
    }

    @Test
    fun evaluateUiVisibilityRequiresInteractiveAndUnlockedScreen() {
        assertTrue(ForegroundStateTracker.evaluateUiVisibility(inForeground = true, isInteractive = true, isKeyguardLocked = false))
        assertFalse(ForegroundStateTracker.evaluateUiVisibility(inForeground = false, isInteractive = true, isKeyguardLocked = false))
        assertFalse(ForegroundStateTracker.evaluateUiVisibility(inForeground = true, isInteractive = false, isKeyguardLocked = false))
        assertFalse(ForegroundStateTracker.evaluateUiVisibility(inForeground = true, isInteractive = true, isKeyguardLocked = true))
    }
}
