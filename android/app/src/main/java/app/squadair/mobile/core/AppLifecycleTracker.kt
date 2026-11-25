package app.squadair.mobile.core

import android.app.Activity
import android.app.Application
import android.os.Bundle
import android.os.Handler
import android.os.Looper

class ForegroundStateTracker(
    private val onForegroundChanged: (Boolean) -> Unit,
) {
    private val startedActivityIds = mutableSetOf<Int>()
    private val pendingConfigActivityIds = mutableSetOf<Int>()

    var isInForeground: Boolean = false
        private set

    fun onActivityStarted(activityId: Int) {
        if (pendingConfigActivityIds.isNotEmpty()) {
            val first = pendingConfigActivityIds.first()
            pendingConfigActivityIds.remove(first)
        }
        startedActivityIds.add(activityId)
        updateForeground()
    }

    fun onActivityStopped(activityId: Int, isChangingConfigurations: Boolean) {
        startedActivityIds.remove(activityId)
        if (isChangingConfigurations) {
            pendingConfigActivityIds.add(activityId)
        } else {
            pendingConfigActivityIds.remove(activityId)
        }
        updateForeground()
    }

    fun onActivityDestroyed(activityId: Int, isChangingConfigurations: Boolean = false) {
        startedActivityIds.remove(activityId)
        if (!isChangingConfigurations) {
            pendingConfigActivityIds.remove(activityId)
        }
        updateForeground()
    }

    fun expirePendingConfigChange(activityId: Int) {
        if (pendingConfigActivityIds.remove(activityId)) {
            updateForeground()
        }
    }

    fun hasPendingConfigChanges(): Boolean = pendingConfigActivityIds.isNotEmpty()

    private fun updateForeground() {
        val next = startedActivityIds.isNotEmpty() || pendingConfigActivityIds.isNotEmpty()
        if (next != isInForeground) {
            isInForeground = next
            onForegroundChanged(next)
        }
    }

    companion object {
        fun evaluateUiVisibility(
            inForeground: Boolean,
            isInteractive: Boolean,
            isKeyguardLocked: Boolean,
        ): Boolean = inForeground && isInteractive && !isKeyguardLocked
    }
}

class AppLifecycleTracker(
    onForegroundChanged: (Boolean) -> Unit,
    private val onUiVisibilityCheckRequested: () -> Unit = {},
) : Application.ActivityLifecycleCallbacks {
    private val mainHandler = Handler(Looper.getMainLooper())
    private val pendingExpiryRunnables = mutableMapOf<Int, Runnable>()
    private val tracker = ForegroundStateTracker(onForegroundChanged)

    val isInForeground: Boolean
        get() = tracker.isInForeground

    override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) = Unit

    override fun onActivityStarted(activity: Activity) {
        tracker.onActivityStarted(System.identityHashCode(activity))
        if (!tracker.hasPendingConfigChanges() && pendingExpiryRunnables.isNotEmpty()) {
            pendingExpiryRunnables.values.forEach { mainHandler.removeCallbacks(it) }
            pendingExpiryRunnables.clear()
        }
    }

    override fun onActivityResumed(activity: Activity) {
        onUiVisibilityCheckRequested()
    }

    override fun onActivityPaused(activity: Activity) = Unit

    override fun onActivityStopped(activity: Activity) {
        val id = System.identityHashCode(activity)
        val willRecreate = activity.isChangingConfigurations && !activity.isFinishing
        tracker.onActivityStopped(
            activityId = id,
            isChangingConfigurations = willRecreate,
        )
        if (willRecreate) {
            pendingExpiryRunnables.remove(id)?.let { mainHandler.removeCallbacks(it) }
            val expiry = Runnable {
                pendingExpiryRunnables.remove(id)
                tracker.expirePendingConfigChange(id)
            }
            pendingExpiryRunnables[id] = expiry
            mainHandler.postDelayed(expiry, CONFIG_CHANGE_GRACE_MS)
        }
    }

    override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) = Unit

    override fun onActivityDestroyed(activity: Activity) {
        val id = System.identityHashCode(activity)
        val willRecreate = activity.isChangingConfigurations && !activity.isFinishing
        tracker.onActivityDestroyed(
            activityId = id,
            isChangingConfigurations = willRecreate,
        )
        if (!willRecreate) {
            pendingExpiryRunnables.remove(id)?.let { mainHandler.removeCallbacks(it) }
        }
    }

    private companion object {
        const val CONFIG_CHANGE_GRACE_MS = 750L
    }
}
