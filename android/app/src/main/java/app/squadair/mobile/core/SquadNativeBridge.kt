package app.squadair.mobile.core

import android.content.Context
import android.view.Surface
import java.util.concurrent.atomic.AtomicBoolean

object SquadNativeBridge {
    private val librariesLoaded = AtomicBoolean(false)
    private val contextInitialized = AtomicBoolean(false)

    private fun ensureLibrariesLoaded() {
        if (librariesLoaded.compareAndSet(false, true)) {
            try {
                System.loadLibrary("c++_shared")
            } catch (_: UnsatisfiedLinkError) {
            }
            System.loadLibrary("squad_core")
        }
    }

    fun ensureInitialized(context: Context) {
        ensureLibrariesLoaded()
        if (contextInitialized.compareAndSet(false, true)) {
            nativeInitContext(context.applicationContext)
        }
    }

    @JvmStatic
    external fun nativeInitContext(context: Context)

    @JvmStatic
    external fun nativeAttachSurface(surface: Surface)

    @JvmStatic
    external fun nativeDetachSurface()
}
