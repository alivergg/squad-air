package app.squadair.mobile.core

import app.squadair.mobile.service.AudioRoute
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

private class FakeAecPreferenceStore(
    private var baseline: Boolean? = null,
    private var overrideActive: Boolean = false,
) : AecPreferenceStore {
    override fun getBaselineAec(): Boolean? = baseline
    override fun setBaselineAec(enabled: Boolean) {
        baseline = enabled
    }

    override fun isSpeakerOverrideActive(): Boolean = overrideActive
    override fun setSpeakerOverrideActive(active: Boolean) {
        overrideActive = active
    }
}

class SpeakerAecStateMachineTest {
    @Test
    fun baselineFalse_speakerEnablesAndEarpieceRestoresFalse() {
        val store = FakeAecPreferenceStore()
        val sm = SpeakerAecStateMachine(store)
        sm.reconcileStartup(persistedCoreAec = false)

        val enterSpeaker = sm.onRouteChanged(AudioRoute.SPEAKER, isCallActive = true, currentEffectiveAec = false)
        assertTrue(enterSpeaker.effectiveAec)
        assertTrue(enterSpeaker.shouldApply)
        assertTrue(enterSpeaker.isSpeakerOverrideActive)
        assertEquals(false, store.getBaselineAec())

        val switchEarpiece = sm.onRouteChanged(AudioRoute.EARPIECE, isCallActive = true, currentEffectiveAec = true)
        assertFalse(switchEarpiece.effectiveAec)
        assertTrue(switchEarpiece.shouldApply)
        assertFalse(switchEarpiece.isSpeakerOverrideActive)
    }

    @Test
    fun baselineFalse_speakerToWiredOrBluetoothRestoresFalse() {
        val store = FakeAecPreferenceStore()
        val sm = SpeakerAecStateMachine(store)
        sm.reconcileStartup(false)

        sm.onRouteChanged(AudioRoute.SPEAKER, isCallActive = true, currentEffectiveAec = false)
        val switchWired = sm.onRouteChanged(AudioRoute.WIRED_HEADSET, isCallActive = true, currentEffectiveAec = true)
        assertFalse(switchWired.effectiveAec)
        assertTrue(switchWired.shouldApply)

        sm.onRouteChanged(AudioRoute.SPEAKER, isCallActive = true, currentEffectiveAec = false)
        val switchBt = sm.onRouteChanged(AudioRoute.BLUETOOTH, isCallActive = true, currentEffectiveAec = true)
        assertFalse(switchBt.effectiveAec)
        assertTrue(switchBt.shouldApply)
    }

    @Test
    fun baselineTrue_speakerStaysTrueAndEarpieceRestoresTrue() {
        val store = FakeAecPreferenceStore()
        val sm = SpeakerAecStateMachine(store)
        sm.reconcileStartup(persistedCoreAec = true)

        val enterSpeaker = sm.onRouteChanged(AudioRoute.SPEAKER, isCallActive = true, currentEffectiveAec = true)
        assertTrue(enterSpeaker.effectiveAec)
        assertFalse(enterSpeaker.shouldApply)
        assertTrue(enterSpeaker.isSpeakerOverrideActive)

        val switchEarpiece = sm.onRouteChanged(AudioRoute.EARPIECE, isCallActive = true, currentEffectiveAec = true)
        assertTrue(switchEarpiece.effectiveAec)
        assertFalse(switchEarpiece.shouldApply)
        assertFalse(switchEarpiece.isSpeakerOverrideActive)
    }

    @Test
    fun callEndedWhileOnSpeaker_restoresBaselineFalse() {
        val store = FakeAecPreferenceStore()
        val sm = SpeakerAecStateMachine(store)
        sm.reconcileStartup(false)

        sm.onRouteChanged(AudioRoute.SPEAKER, isCallActive = true, currentEffectiveAec = false)
        val ended = sm.onCallEnded(currentEffectiveAec = true)
        assertFalse(ended.effectiveAec)
        assertTrue(ended.shouldApply)
        assertFalse(ended.isSpeakerOverrideActive)
    }

    @Test
    fun startupCrashRecovery_restoresSavedBaselineAndClearsOverride() {
        val store = FakeAecPreferenceStore(baseline = false, overrideActive = true)
        val sm = SpeakerAecStateMachine(store)

        val recovered = sm.reconcileStartup(persistedCoreAec = true)
        assertFalse(recovered.effectiveAec)
        assertTrue(recovered.shouldApply)
        assertFalse(recovered.isSpeakerOverrideActive)
        assertFalse(store.isSpeakerOverrideActive())
    }

    @Test
    fun manualToggleOffWhileOnSpeaker_persistsNewBaselineAndDoesNotRevert() {
        val store = FakeAecPreferenceStore()
        val sm = SpeakerAecStateMachine(store)
        sm.reconcileStartup(false)

        sm.onRouteChanged(AudioRoute.SPEAKER, isCallActive = true, currentEffectiveAec = false)
        val toggled = sm.onUserToggledAec(false)
        assertFalse(toggled.effectiveAec)
        assertTrue(toggled.shouldApply)
        assertFalse(toggled.isSpeakerOverrideActive)

        val toEarpiece = sm.onRouteChanged(AudioRoute.EARPIECE, isCallActive = true, currentEffectiveAec = false)
        assertFalse(toEarpiece.effectiveAec)
        assertFalse(toEarpiece.shouldApply)

        val reenterSpeaker = sm.onRouteChanged(AudioRoute.SPEAKER, isCallActive = true, currentEffectiveAec = false)
        assertTrue(reenterSpeaker.effectiveAec)
        assertTrue(reenterSpeaker.shouldApply)
        assertEquals(false, store.getBaselineAec())
    }

    @Test
    fun manualToggleOnWhileOnSpeaker_updatesBaselineToTrue() {
        val store = FakeAecPreferenceStore()
        val sm = SpeakerAecStateMachine(store)
        sm.reconcileStartup(false)

        sm.onRouteChanged(AudioRoute.SPEAKER, isCallActive = true, currentEffectiveAec = false)
        val toggled = sm.onUserToggledAec(true)
        assertTrue(toggled.effectiveAec)
        assertFalse(toggled.isSpeakerOverrideActive)

        val toEarpiece = sm.onRouteChanged(AudioRoute.EARPIECE, isCallActive = true, currentEffectiveAec = true)
        assertTrue(toEarpiece.effectiveAec)
        assertFalse(toEarpiece.shouldApply)
    }

    @Test
    fun preCallRouteChange_doesNotMutateAec() {
        val store = FakeAecPreferenceStore()
        val sm = SpeakerAecStateMachine(store)
        sm.reconcileStartup(false)

        val effect = sm.onRouteChanged(AudioRoute.SPEAKER, isCallActive = false, currentEffectiveAec = false)
        assertFalse(effect.effectiveAec)
        assertFalse(effect.shouldApply)
        assertFalse(effect.isSpeakerOverrideActive)
    }
}
