package app.squadair.mobile.core

import android.content.Context
import android.content.SharedPreferences
import app.squadair.mobile.service.AudioRoute

interface AecPreferenceStore {
    fun getBaselineAec(): Boolean?
    fun setBaselineAec(enabled: Boolean)
    fun isSpeakerOverrideActive(): Boolean
    fun setSpeakerOverrideActive(active: Boolean)
}

data class AecEffect(
    val effectiveAec: Boolean,
    val shouldApply: Boolean,
    val isSpeakerOverrideActive: Boolean,
)

class SpeakerAecStateMachine(
    private val store: AecPreferenceStore,
) {
    fun reconcileStartup(persistedCoreAec: Boolean): AecEffect {
        if (store.isSpeakerOverrideActive()) {
            val baseline = store.getBaselineAec() ?: false
            store.setSpeakerOverrideActive(false)
            return AecEffect(
                effectiveAec = baseline,
                shouldApply = persistedCoreAec != baseline,
                isSpeakerOverrideActive = false,
            )
        }
        store.setBaselineAec(persistedCoreAec)
        return AecEffect(
            effectiveAec = persistedCoreAec,
            shouldApply = false,
            isSpeakerOverrideActive = false,
        )
    }

    fun onRouteChanged(
        route: AudioRoute,
        isCallActive: Boolean,
        currentEffectiveAec: Boolean,
    ): AecEffect {
        if (!isCallActive) {
            return AecEffect(
                effectiveAec = currentEffectiveAec,
                shouldApply = false,
                isSpeakerOverrideActive = store.isSpeakerOverrideActive(),
            )
        }
        return if (route == AudioRoute.SPEAKER) {
            if (!store.isSpeakerOverrideActive()) {
                store.setBaselineAec(currentEffectiveAec)
                store.setSpeakerOverrideActive(true)
            }
            AecEffect(
                effectiveAec = true,
                shouldApply = !currentEffectiveAec,
                isSpeakerOverrideActive = true,
            )
        } else {
            if (store.isSpeakerOverrideActive()) {
                val baseline = store.getBaselineAec() ?: currentEffectiveAec
                store.setSpeakerOverrideActive(false)
                AecEffect(
                    effectiveAec = baseline,
                    shouldApply = currentEffectiveAec != baseline,
                    isSpeakerOverrideActive = false,
                )
            } else {
                AecEffect(
                    effectiveAec = currentEffectiveAec,
                    shouldApply = false,
                    isSpeakerOverrideActive = false,
                )
            }
        }
    }

    fun onCallEnded(currentEffectiveAec: Boolean): AecEffect {
        if (store.isSpeakerOverrideActive()) {
            val baseline = store.getBaselineAec() ?: false
            store.setSpeakerOverrideActive(false)
            return AecEffect(
                effectiveAec = baseline,
                shouldApply = currentEffectiveAec != baseline,
                isSpeakerOverrideActive = false,
            )
        }
        return AecEffect(
            effectiveAec = currentEffectiveAec,
            shouldApply = false,
            isSpeakerOverrideActive = false,
        )
    }

    fun onUserToggledAec(enabled: Boolean): AecEffect {
        store.setBaselineAec(enabled)
        store.setSpeakerOverrideActive(false)
        return AecEffect(
            effectiveAec = enabled,
            shouldApply = true,
            isSpeakerOverrideActive = false,
        )
    }
}

class SharedPrefsAecPreferenceStore(context: Context) : AecPreferenceStore {
    private val prefs: SharedPreferences = context.applicationContext.getSharedPreferences(
        PREFS_NAME,
        Context.MODE_PRIVATE,
    )

    override fun getBaselineAec(): Boolean? {
        return if (prefs.contains(KEY_BASELINE_AEC)) {
            prefs.getBoolean(KEY_BASELINE_AEC, false)
        } else {
            null
        }
    }

    override fun setBaselineAec(enabled: Boolean) {
        prefs.edit().putBoolean(KEY_BASELINE_AEC, enabled).apply()
    }

    override fun isSpeakerOverrideActive(): Boolean {
        return prefs.getBoolean(KEY_SPEAKER_OVERRIDE_ACTIVE, false)
    }

    override fun setSpeakerOverrideActive(active: Boolean) {
        prefs.edit().putBoolean(KEY_SPEAKER_OVERRIDE_ACTIVE, active).apply()
    }

    private companion object {
        const val PREFS_NAME = "squadair_audio_prefs"
        const val KEY_BASELINE_AEC = "baseline_aec_enabled"
        const val KEY_SPEAKER_OVERRIDE_ACTIVE = "speaker_aec_override_active"
    }
}
