package app.squadair.mobile.service

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioAttributes
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import androidx.annotation.RequiresApi
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executor
import java.util.concurrent.TimeUnit

class AudioRouteController(
    context: Context,
    private val onRouteChanged: (AudioRoute, List<AudioRoute>) -> Unit,
    private val onHardwareRouteSwitched: (String?, String?) -> Unit,
) {
    private val appContext = context.applicationContext
    private val audioManager = appContext.getSystemService(Context.AUDIO_SERVICE) as AudioManager
    private val mainHandler = Handler(Looper.getMainLooper())
    private val operationLock = Any()
    private val stateLock = Any()

    private var focusRequest: AudioFocusRequest? = null
    private var active = false
    private var sessionGeneration = 0L
    private var currentRoute: AudioRoute = AudioRoute.SPEAKER
    private var lastKnownAvailableRoutes: List<AudioRoute> = emptyList()
    private var lastAppliedSpecs: Pair<String?, String?>? = null

    private var pendingTargetDeviceId: Int? = null
    private var pendingRoute: AudioRoute? = null
    private var pendingConfirmRunnable: Runnable? = null

    private var scoStartPending = false
    private var pendingScoTimeoutRunnable: Runnable? = null
    private var scoReceiverRegistered = false
    private var activeDeviceCallback: AudioDeviceCallback? = null
    private var activeCommListener: AudioManager.OnCommunicationDeviceChangedListener? = null
    private var initialRouteLatch: CountDownLatch? = null

    private val scoStateReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            if (intent?.action != AudioManager.ACTION_SCO_AUDIO_STATE_UPDATED) return
            val state = intent.getIntExtra(
                AudioManager.EXTRA_SCO_AUDIO_STATE,
                AudioManager.SCO_AUDIO_STATE_ERROR,
            )
            val prevState = intent.getIntExtra(
                AudioManager.EXTRA_SCO_AUDIO_PREVIOUS_STATE,
                AudioManager.SCO_AUDIO_STATE_DISCONNECTED,
            )
            handleScoAudioStateUpdated(state, prevState)
        }
    }

    fun isActive(): Boolean = synchronized(stateLock) { active }

    fun activate() {
        synchronized(operationLock) {
            val alreadyActive = synchronized(stateLock) { active }
            if (alreadyActive) return

            sessionGeneration += 1L
            val gen = sessionGeneration
            synchronized(stateLock) {
                active = true
                initialRouteLatch = CountDownLatch(1)
            }

            val attrs = AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                .build()

            val req = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
                .setAudioAttributes(attrs)
                .setAcceptsDelayedFocusGain(false)
                .setWillPauseWhenDucked(false)
                .build()
            focusRequest = req
            audioManager.requestAudioFocus(req)

            audioManager.mode = AudioManager.MODE_IN_COMMUNICATION

            val devCallback = object : AudioDeviceCallback() {
                override fun onAudioDevicesAdded(addedDevices: Array<out AudioDeviceInfo>?) {
                    val snapshots = addedDevices?.map { toSnapshot(it) }.orEmpty()
                    reconcileRoutesOnHardwareChange(
                        added = true,
                        addedDevices = snapshots,
                        expectedGen = gen,
                    )
                }

                override fun onAudioDevicesRemoved(removedDevices: Array<out AudioDeviceInfo>?) {
                    reconcileRoutesOnHardwareChange(
                        added = false,
                        addedDevices = emptyList(),
                        expectedGen = gen,
                    )
                }
            }
            activeDeviceCallback = devCallback
            audioManager.registerAudioDeviceCallback(devCallback, mainHandler)

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                val listener = AudioManager.OnCommunicationDeviceChangedListener { device ->
                    handleCommunicationDeviceChanged(device, expectedGen = gen)
                }
                try {
                    audioManager.addOnCommunicationDeviceChangedListener(
                        Executor { runnable -> mainHandler.post(runnable) },
                        listener,
                    )
                    activeCommListener = listener
                } catch (_: Throwable) {
                }
            } else {
                try {
                    appContext.registerReceiver(
                        scoStateReceiver,
                        IntentFilter(AudioManager.ACTION_SCO_AUDIO_STATE_UPDATED),
                    )
                    scoReceiverRegistered = true
                } catch (_: Throwable) {
                }
            }

            val available = computeAvailableRoutes()
            lastKnownAvailableRoutes = available
            val initial = AudioRoutePlanner.pickInitialRoute(available)
            applyRouteInternalLocked(initial, notifyRust = true, expectedGen = gen)
        }
    }

    fun awaitInitialRouteSettled(timeoutMs: Long = DEFAULT_SETTLE_TIMEOUT_MS) {
        if (Looper.myLooper() == Looper.getMainLooper()) return
        val latch = synchronized(stateLock) {
            if (active) initialRouteLatch else null
        } ?: return
        try {
            latch.await(timeoutMs, TimeUnit.MILLISECONDS)
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
        }
    }

    fun selectRoute(route: AudioRoute) {
        synchronized(operationLock) {
            val currentlyActive = synchronized(stateLock) { active }
            if (!currentlyActive) {
                currentRoute = route
                val available = computeAvailableRoutes()
                lastKnownAvailableRoutes = available
                onRouteChanged(route, available)
                return
            }
            applyRouteInternalLocked(route, notifyRust = true, expectedGen = sessionGeneration)
        }
    }

    fun deactivate() {
        synchronized(operationLock) {
            val wasActive = synchronized(stateLock) { active }
            if (!wasActive) return

            sessionGeneration += 1L
            synchronized(stateLock) {
                active = false
                signalInitialRouteSettledLocked()
            }

            lastAppliedSpecs = null
            lastKnownAvailableRoutes = emptyList()
            pendingTargetDeviceId = null
            pendingRoute = null
            scoStartPending = false

            val confirmRunnable = pendingConfirmRunnable.also { pendingConfirmRunnable = null }
            val scoRunnable = pendingScoTimeoutRunnable.also { pendingScoTimeoutRunnable = null }
            val focusToAbandon = focusRequest.also { focusRequest = null }
            val devCallback = activeDeviceCallback.also { activeDeviceCallback = null }
            val commListener = activeCommListener.also { activeCommListener = null }
            val wasScoRegistered = scoReceiverRegistered.also { scoReceiverRegistered = false }

            confirmRunnable?.let { mainHandler.removeCallbacks(it) }
            scoRunnable?.let { mainHandler.removeCallbacks(it) }

            devCallback?.let { cb ->
                try {
                    audioManager.unregisterAudioDeviceCallback(cb)
                } catch (_: Throwable) {
                }
            }

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                commListener?.let { listener ->
                    try {
                        audioManager.removeOnCommunicationDeviceChangedListener(listener)
                    } catch (_: Throwable) {
                    }
                }
                try {
                    audioManager.clearCommunicationDevice()
                } catch (_: Throwable) {
                }
            } else {
                if (wasScoRegistered) {
                    try {
                        appContext.unregisterReceiver(scoStateReceiver)
                    } catch (_: Throwable) {
                    }
                }
                @Suppress("DEPRECATION")
                try {
                    if (audioManager.isBluetoothScoOn) {
                        audioManager.stopBluetoothSco()
                        audioManager.isBluetoothScoOn = false
                    }
                    audioManager.isSpeakerphoneOn = false
                } catch (_: Throwable) {
                }
            }

            try {
                audioManager.mode = AudioManager.MODE_NORMAL
            } catch (_: Throwable) {
            }
            focusToAbandon?.let { req ->
                try {
                    audioManager.abandonAudioFocusRequest(req)
                } catch (_: Throwable) {
                }
            }
            onHardwareRouteSwitched(null, null)
        }
    }

    private fun isGenerationActiveLocked(expectedGen: Long): Boolean {
        return synchronized(stateLock) { active } && sessionGeneration == expectedGen
    }

    private fun reconcileRoutesOnHardwareChange(
        added: Boolean,
        addedDevices: List<AudioDeviceSnapshot>,
        expectedGen: Long,
    ) {
        synchronized(operationLock) {
            if (!isGenerationActiveLocked(expectedGen)) return
            val prevAvailable = lastKnownAvailableRoutes
            val available = computeAvailableRoutes()
            val isApi31Plus = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S
            val btAddedInDevices = added && addedDevices.any {
                AudioRoutePlanner.isBluetoothRouteDevice(it, isApi31Plus)
            }
            val wiredAddedInDevices = added && addedDevices.any {
                AudioRoutePlanner.isWiredRouteDevice(it)
            }

            val prevRoute = currentRoute
            val target = AudioRoutePlanner.reconcileRouteOnHardwareChange(
                currentRoute = prevRoute,
                available = available,
                added = added,
                previousAvailable = prevAvailable,
                addedDevices = addedDevices,
                isApi31Plus = isApi31Plus,
            )

            lastKnownAvailableRoutes = if (added) {
                available.filter { route ->
                    when (route) {
                        AudioRoute.BLUETOOTH ->
                            prevAvailable.contains(AudioRoute.BLUETOOTH) || btAddedInDevices
                        AudioRoute.WIRED_HEADSET ->
                            prevAvailable.contains(AudioRoute.WIRED_HEADSET) || wiredAddedInDevices
                        else -> true
                    }
                }
            } else {
                available
            }

            if (target != prevRoute) {
                applyRouteInternalLocked(target, notifyRust = true, expectedGen = expectedGen)
            } else {
                onRouteChanged(prevRoute, available)
                val activeCommType = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                    try {
                        audioManager.communicationDevice?.type
                    } catch (_: Throwable) {
                        null
                    }
                } else {
                    null
                }
                applyHardwareSpecsIfChangedLocked(
                    route = prevRoute,
                    activeCommDeviceType = activeCommType,
                    notifyRust = true,
                    expectedGen = expectedGen,
                )
            }
        }
    }

    private fun applyRouteInternalLocked(
        route: AudioRoute,
        notifyRust: Boolean,
        expectedGen: Long,
    ) {
        if (!isGenerationActiveLocked(expectedGen)) return
        val available = computeAvailableRoutes()
        lastKnownAvailableRoutes = available
        val resolved = if (available.contains(route)) route else AudioRoute.SPEAKER

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            applyRouteApi31PlusLocked(resolved, available, notifyRust, expectedGen)
        } else {
            applyRouteLegacyLocked(resolved, available, notifyRust, expectedGen)
        }
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private fun applyRouteApi31PlusLocked(
        resolved: AudioRoute,
        available: List<AudioRoute>,
        notifyRust: Boolean,
        expectedGen: Long,
    ) {
        cancelPendingConfirmRunnableLocked()

        val commDevices = audioManager.availableCommunicationDevices
        val snapshots = commDevices.map { toSnapshot(it) }
        var targetId = AudioRoutePlanner.selectCommunicationDeviceId(resolved, snapshots)
        var targetDevice = targetId?.let { id -> commDevices.firstOrNull { it.id == id } }

        if (targetDevice == null) {
            val fallbackRoute = AudioRoutePlanner.pickInitialRoute(available.filter { it != resolved })
            targetId = AudioRoutePlanner.selectCommunicationDeviceId(fallbackRoute, snapshots)
            targetDevice = targetId?.let { id -> commDevices.firstOrNull { it.id == id } }
        }

        if (targetDevice == null) {
            currentRoute = AudioRoute.SPEAKER
            pendingTargetDeviceId = null
            pendingRoute = null
            onRouteChanged(AudioRoute.SPEAKER, available)
            applyHardwareSpecsIfChangedLocked(AudioRoute.SPEAKER, null, notifyRust, expectedGen)
            synchronized(stateLock) { signalInitialRouteSettledLocked() }
            return
        }

        val effectiveRoute = AudioRoutePlanner.mapCommunicationDeviceTypeToRoute(targetDevice.type)
            ?: resolved

        val currentComm = try {
            audioManager.communicationDevice
        } catch (_: Throwable) {
            null
        }

        if (currentComm?.id == targetDevice.id) {
            currentRoute = effectiveRoute
            pendingTargetDeviceId = null
            pendingRoute = null
            onRouteChanged(effectiveRoute, available)
            applyHardwareSpecsIfChangedLocked(effectiveRoute, targetDevice.type, notifyRust, expectedGen)
            synchronized(stateLock) { signalInitialRouteSettledLocked() }
            return
        }

        currentRoute = effectiveRoute
        pendingRoute = effectiveRoute
        pendingTargetDeviceId = targetDevice.id

        val accepted = try {
            audioManager.setCommunicationDevice(targetDevice)
        } catch (_: Throwable) {
            false
        }

        if (!accepted) {
            val actual = try {
                audioManager.communicationDevice
            } catch (_: Throwable) {
                null
            }
            val actualRoute = AudioRoutePlanner.mapCommunicationDeviceTypeToRoute(actual?.type)
                ?: AudioRoute.SPEAKER
            currentRoute = actualRoute
            pendingTargetDeviceId = null
            pendingRoute = null
            val latestAvailable = computeAvailableRoutes()
            lastKnownAvailableRoutes = latestAvailable
            onRouteChanged(actualRoute, latestAvailable)
            applyHardwareSpecsIfChangedLocked(actualRoute, actual?.type, notifyRust, expectedGen)
            synchronized(stateLock) { signalInitialRouteSettledLocked() }
            return
        }

        onRouteChanged(effectiveRoute, available)

        val expectedId = targetDevice.id
        val expectedType = targetDevice.type
        val timeoutRunnable = Runnable {
            synchronized(operationLock) {
                if (!isGenerationActiveLocked(expectedGen) || pendingTargetDeviceId != expectedId) {
                    return@synchronized
                }
                pendingTargetDeviceId = null
                pendingRoute = null
                pendingConfirmRunnable = null

                val actual = try {
                    audioManager.communicationDevice
                } catch (_: Throwable) {
                    null
                }
                val mappedActual = AudioRoutePlanner.mapCommunicationDeviceTypeToRoute(actual?.type)
                val finalRoute = mappedActual ?: effectiveRoute
                val routeChangedFromExpected = mappedActual != null && mappedActual != effectiveRoute
                if (routeChangedFromExpected) {
                    currentRoute = finalRoute
                    val latestAvailable = computeAvailableRoutes()
                    lastKnownAvailableRoutes = latestAvailable
                    onRouteChanged(finalRoute, latestAvailable)
                }
                if (notifyRust) {
                    applyHardwareSpecsIfChangedLocked(
                        route = finalRoute,
                        activeCommDeviceType = actual?.type ?: expectedType,
                        notifyRust = true,
                        expectedGen = expectedGen,
                    )
                }
                synchronized(stateLock) { signalInitialRouteSettledLocked() }
            }
        }

        pendingConfirmRunnable = timeoutRunnable
        mainHandler.postDelayed(timeoutRunnable, COMM_DEVICE_CONFIRM_TIMEOUT_MS)
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private fun handleCommunicationDeviceChanged(
        device: AudioDeviceInfo?,
        expectedGen: Long,
    ) {
        synchronized(operationLock) {
            if (!isGenerationActiveLocked(expectedGen)) return
            val targetId = pendingTargetDeviceId
            if (targetId != null) {
                if (device?.id != targetId) {
                    return
                }
                val expectedRoute = pendingRoute ?: currentRoute
                pendingConfirmRunnable?.let { mainHandler.removeCallbacks(it) }
                pendingConfirmRunnable = null
                pendingTargetDeviceId = null
                pendingRoute = null
                currentRoute = expectedRoute
                applyHardwareSpecsIfChangedLocked(
                    route = expectedRoute,
                    activeCommDeviceType = device.type,
                    notifyRust = true,
                    expectedGen = expectedGen,
                )
                synchronized(stateLock) { signalInitialRouteSettledLocked() }
            } else if (device != null) {
                val mapped = AudioRoutePlanner.mapCommunicationDeviceTypeToRoute(device.type)
                val avail = computeAvailableRoutes()
                lastKnownAvailableRoutes = avail
                if (mapped != null && mapped != currentRoute && avail.contains(mapped)) {
                    currentRoute = mapped
                    onRouteChanged(mapped, avail)
                }
                applyHardwareSpecsIfChangedLocked(
                    route = currentRoute,
                    activeCommDeviceType = device.type,
                    notifyRust = true,
                    expectedGen = expectedGen,
                )
            }
        }
    }

    @Suppress("DEPRECATION")
    private fun applyRouteLegacyLocked(
        resolved: AudioRoute,
        available: List<AudioRoute>,
        notifyRust: Boolean,
        expectedGen: Long,
    ) {
        cancelPendingScoTimeoutRunnableLocked()

        when (resolved) {
            AudioRoute.BLUETOOTH -> {
                currentRoute = AudioRoute.BLUETOOTH
                audioManager.isSpeakerphoneOn = false
                val alreadySco = audioManager.isBluetoothScoOn
                if (!alreadySco) {
                    scoStartPending = true
                    val started = try {
                        audioManager.startBluetoothSco()
                        audioManager.isBluetoothScoOn = true
                        true
                    } catch (_: Throwable) {
                        scoStartPending = false
                        false
                    }
                    if (!started) {
                        fallbackFromFailedBluetoothScoLocked(expectedGen)
                        return
                    }
                }
                onRouteChanged(AudioRoute.BLUETOOTH, available)
                if (alreadySco) {
                    scoStartPending = false
                    applyHardwareSpecsIfChangedLocked(
                        route = AudioRoute.BLUETOOTH,
                        activeCommDeviceType = AudioDeviceInfo.TYPE_BLUETOOTH_SCO,
                        notifyRust = notifyRust,
                        expectedGen = expectedGen,
                    )
                    synchronized(stateLock) { signalInitialRouteSettledLocked() }
                } else {
                    val timeoutRunnable = Runnable {
                        synchronized(operationLock) {
                            if (!isGenerationActiveLocked(expectedGen) || !scoStartPending) {
                                return@synchronized
                            }
                            scoStartPending = false
                            pendingScoTimeoutRunnable = null
                            if (audioManager.isBluetoothScoOn) {
                                if (notifyRust) {
                                    applyHardwareSpecsIfChangedLocked(
                                        route = AudioRoute.BLUETOOTH,
                                        activeCommDeviceType = AudioDeviceInfo.TYPE_BLUETOOTH_SCO,
                                        notifyRust = true,
                                        expectedGen = expectedGen,
                                    )
                                }
                                synchronized(stateLock) { signalInitialRouteSettledLocked() }
                            } else {
                                fallbackFromFailedBluetoothScoLocked(expectedGen)
                            }
                        }
                    }
                    pendingScoTimeoutRunnable = timeoutRunnable
                    mainHandler.postDelayed(timeoutRunnable, SCO_CONNECT_TIMEOUT_MS)
                }
            }
            AudioRoute.WIRED_HEADSET, AudioRoute.EARPIECE, AudioRoute.SPEAKER -> {
                scoStartPending = false
                currentRoute = resolved
                if (audioManager.isBluetoothScoOn) {
                    try {
                        audioManager.stopBluetoothSco()
                        audioManager.isBluetoothScoOn = false
                    } catch (_: Throwable) {
                    }
                }
                audioManager.isSpeakerphoneOn = (resolved == AudioRoute.SPEAKER)
                onRouteChanged(resolved, available)
                applyHardwareSpecsIfChangedLocked(resolved, null, notifyRust, expectedGen)
                synchronized(stateLock) { signalInitialRouteSettledLocked() }
            }
        }
    }

    @Suppress("DEPRECATION")
    private fun handleScoAudioStateUpdated(state: Int, prevState: Int) {
        synchronized(operationLock) {
            val gen = sessionGeneration
            if (!synchronized(stateLock) { active } || !scoReceiverRegistered) return
            when (state) {
                AudioManager.SCO_AUDIO_STATE_CONNECTED -> {
                    if (currentRoute != AudioRoute.BLUETOOTH) return
                    pendingScoTimeoutRunnable?.let { mainHandler.removeCallbacks(it) }
                    pendingScoTimeoutRunnable = null
                    scoStartPending = false
                    applyHardwareSpecsIfChangedLocked(
                        route = AudioRoute.BLUETOOTH,
                        activeCommDeviceType = AudioDeviceInfo.TYPE_BLUETOOTH_SCO,
                        notifyRust = true,
                        expectedGen = gen,
                    )
                    synchronized(stateLock) { signalInitialRouteSettledLocked() }
                }
                AudioManager.SCO_AUDIO_STATE_DISCONNECTED -> {
                    val shouldFallback = currentRoute == AudioRoute.BLUETOOTH &&
                        (prevState == AudioManager.SCO_AUDIO_STATE_CONNECTED ||
                            prevState == AudioManager.SCO_AUDIO_STATE_CONNECTING)
                    if (shouldFallback) {
                        fallbackFromFailedBluetoothScoLocked(gen)
                    }
                }
            }
        }
    }

    private fun fallbackFromFailedBluetoothScoLocked(expectedGen: Long) {
        if (!isGenerationActiveLocked(expectedGen)) return
        val availableWithoutBt = computeAvailableRoutes().filter { it != AudioRoute.BLUETOOTH }
        val fallback = when {
            availableWithoutBt.contains(AudioRoute.WIRED_HEADSET) -> AudioRoute.WIRED_HEADSET
            availableWithoutBt.contains(AudioRoute.EARPIECE) -> AudioRoute.EARPIECE
            else -> AudioRoute.SPEAKER
        }
        applyRouteInternalLocked(fallback, notifyRust = true, expectedGen = expectedGen)
    }

    private fun applyHardwareSpecsIfChangedLocked(
        route: AudioRoute,
        activeCommDeviceType: Int?,
        notifyRust: Boolean,
        expectedGen: Long,
    ) {
        if (!isGenerationActiveLocked(expectedGen)) return
        val inputSnapshots = audioManager.getDevices(AudioManager.GET_DEVICES_INPUTS).map { toSnapshot(it) }
        val inputSpec = AudioRoutePlanner.resolveInputSpec(
            route = route,
            inputDevices = inputSnapshots,
            activeCommDeviceType = activeCommDeviceType,
        )
        val outputSpec = AudioRoutePlanner.resolveOutputSpec(route)
        val specs = inputSpec to outputSpec

        if (!isGenerationActiveLocked(expectedGen)) return
        if (specs != lastAppliedSpecs) {
            lastAppliedSpecs = specs
            if (notifyRust) {
                onHardwareRouteSwitched(specs.first, specs.second)
            }
        }
    }

    private fun computeAvailableRoutes(): List<AudioRoute> {
        val isApi31Plus = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S
        val devices = if (isApi31Plus) {
            audioManager.availableCommunicationDevices
        } else {
            audioManager.getDevices(AudioManager.GET_DEVICES_OUTPUTS).toList()
        }
        return AudioRoutePlanner.computeAvailableRoutes(
            devices = devices.map { toSnapshot(it) },
            isApi31Plus = isApi31Plus,
        )
    }

    private fun toSnapshot(dev: AudioDeviceInfo): AudioDeviceSnapshot {
        val addr = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            dev.address
        } else {
            ""
        }
        return AudioDeviceSnapshot(
            id = dev.id,
            type = dev.type,
            isSource = dev.isSource,
            isSink = dev.isSink,
            address = addr,
        )
    }

    private fun cancelPendingConfirmRunnableLocked() {
        val runnable = pendingConfirmRunnable.also { pendingConfirmRunnable = null }
        runnable?.let { mainHandler.removeCallbacks(it) }
    }

    private fun cancelPendingScoTimeoutRunnableLocked() {
        val runnable = pendingScoTimeoutRunnable.also { pendingScoTimeoutRunnable = null }
        runnable?.let { mainHandler.removeCallbacks(it) }
    }

    private fun signalInitialRouteSettledLocked() {
        initialRouteLatch?.countDown()
        initialRouteLatch = null
    }

    private companion object {
        const val COMM_DEVICE_CONFIRM_TIMEOUT_MS = 250L
        const val DEFAULT_SETTLE_TIMEOUT_MS = 450L
        const val SCO_CONNECT_TIMEOUT_MS = 1500L
    }
}
