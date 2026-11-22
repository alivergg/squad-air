package app.squadair.mobile.core

import android.content.Context
import android.view.Surface
import android.view.SurfaceHolder
import java.util.concurrent.atomic.AtomicBoolean

object SquadNativeBridge {
    private val librariesLoaded = AtomicBoolean(false)
    private val contextInitialized = AtomicBoolean(false)
    private val surfaceLock = Any()
    private var activeHolder: SurfaceHolder? = null

    private fun ensureLibrariesLoaded() {
        if (librariesLoaded.compareAndSet(false, true)) {
            System.loadLibrary("c++_shared")
            System.loadLibrary("squad_core")
        }
    }

    fun ensureInitialized(context: Context) {
        ensureLibrariesLoaded()
        if (contextInitialized.compareAndSet(false, true)) {
            nativeInitContext(context.applicationContext)
        }
    }

    fun attachSurface(holder: SurfaceHolder) {
        synchronized(surfaceLock) {
            activeHolder = holder
            nativeAttachSurface(holder.surface)
        }
    }

    fun detachSurface(holder: SurfaceHolder) {
        synchronized(surfaceLock) {
            if (activeHolder === holder) {
                activeHolder = null
                nativeDetachSurface()
            }
        }
    }

    @JvmStatic
    external fun nativeInitContext(context: Context)

    @JvmStatic
    external fun nativeAttachSurface(surface: Surface)

    @JvmStatic
    external fun nativeDetachSurface()
}
