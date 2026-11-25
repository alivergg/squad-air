package app.squadair.mobile

import android.app.Application
import app.squadair.mobile.core.AppLifecycleTracker
import app.squadair.mobile.core.SquadNativeBridge
import app.squadair.mobile.core.SquadSessionRepository

class SquadAirApp : Application() {
    override fun onCreate() {
        super.onCreate()
        SquadNativeBridge.ensureInitialized(this)
        val repository = SquadSessionRepository.initialize(this)
        registerActivityLifecycleCallbacks(
            AppLifecycleTracker(
                onForegroundChanged = { inForeground ->
                    repository.onAppForegroundChanged(inForeground)
                },
                onUiVisibilityCheckRequested = {
                    repository.onUiVisibilityPossiblyChanged()
                },
            ),
        )
    }
}
