package app.squadair.mobile.service

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.Build
import android.os.Handler
import android.os.Looper

enum class AudioRoute(val label: String) {
    SPEAKER("Speaker"),
    EARPIECE("Earpiece"),
    BLUETOOTH("Bluetooth"),
    WIRED_HEADSET("Headset"),
}

class AudioRouteController(
    context: Context,
    private val onRouteChanged: (AudioRoute, List<AudioRoute>) -> Unit,
    private val onHardwareRouteSwitched: (String?, String?) -> Unit,
) {
    private val audioManager = context.getSystemService(Context.AUDIO_SERVICE) as AudioManager
    private val mainHandler = Handler(Looper.getMainLooper())
    private var focusRequest: AudioFocusRequest? = null
    private var active = false
    private var currentRoute: AudioRoute = AudioRoute.SPEAKER
    private var lastAppliedSpecs: Pair<String?, String?>? = null

    private val deviceCallback = object : AudioDeviceCallback() {
        override fun onAudioDevicesAdded(addedDevices: Array<out AudioDeviceInfo>?) {
            if (!active) return
            reconcileRoutesOnHardwareChange(added = true)
        }

        override fun onAudioDevicesRemoved(removedDevices: Array<out AudioDeviceInfo>?) {
            if (!active) return
            reconcileRoutesOnHardwareChange(added = false)
        }
    }

    fun activate() {
        if (active) return
        active = true

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
        audioManager.registerAudioDeviceCallback(deviceCallback, mainHandler)

        val available = computeAvailableRoutes()
        val initial = when {
            available.contains(AudioRoute.BLUETOOTH) -> AudioRoute.BLUETOOTH
            available.contains(AudioRoute.WIRED_HEADSET) -> AudioRoute.WIRED_HEADSET
            else -> AudioRoute.SPEAKER
        }
        applyRouteInternal(initial, notifyRust = true)
    }

    fun selectRoute(route: AudioRoute) {
        if (!active) {
            currentRoute = route
            onRouteChanged(route, computeAvailableRoutes())
            return
        }
        applyRouteInternal(route, notifyRust = true)
    }

    fun deactivate() {
        if (!active) return
        active = false
        lastAppliedSpecs = null
        try {
            audioManager.unregisterAudioDeviceCallback(deviceCallback)
        } catch (_: Throwable) {
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            try {
                audioManager.clearCommunicationDevice()
            } catch (_: Throwable) {
            }
        } else {
            @Suppress("DEPRECATION")
            if (audioManager.isBluetoothScoOn) {
                audioManager.stopBluetoothSco()
                audioManager.isBluetoothScoOn = false
            }
            @Suppress("DEPRECATION")
            audioManager.isSpeakerphoneOn = false
        }
        audioManager.mode = AudioManager.MODE_NORMAL
        focusRequest?.let { req ->
            audioManager.abandonAudioFocusRequest(req)
        }
        focusRequest = null
    }

    private fun reconcileRoutesOnHardwareChange(added: Boolean) {
        val available = computeAvailableRoutes()
        val target = when {
            added && available.contains(AudioRoute.BLUETOOTH) -> AudioRoute.BLUETOOTH
            added && available.contains(AudioRoute.WIRED_HEADSET) && currentRoute == AudioRoute.SPEAKER -> AudioRoute.WIRED_HEADSET
            !available.contains(currentRoute) -> {
                when {
                    available.contains(AudioRoute.BLUETOOTH) -> AudioRoute.BLUETOOTH
                    available.contains(AudioRoute.WIRED_HEADSET) -> AudioRoute.WIRED_HEADSET
                    else -> AudioRoute.SPEAKER
                }
            }
            else -> currentRoute
        }
        if (target != currentRoute) {
            applyRouteInternal(target, notifyRust = true)
        } else {
            onRouteChanged(currentRoute, available)
            val specs = resolveDeviceSpecsForRoute(currentRoute)
            if (specs != lastAppliedSpecs) {
                lastAppliedSpecs = specs
                onHardwareRouteSwitched(specs.first, specs.second)
            }
        }
    }

    private fun applyRouteInternal(route: AudioRoute, notifyRust: Boolean) {
        val available = computeAvailableRoutes()
        val resolved = if (available.contains(route)) route else AudioRoute.SPEAKER
        currentRoute = resolved

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            val commDevices = audioManager.availableCommunicationDevices
            val targetDevice = when (resolved) {
                AudioRoute.BLUETOOTH -> commDevices.firstOrNull {
                    it.type == AudioDeviceInfo.TYPE_BLE_HEADSET ||
                        it.type == AudioDeviceInfo.TYPE_BLUETOOTH_SCO
                }
                AudioRoute.WIRED_HEADSET -> commDevices.firstOrNull {
                    it.type == AudioDeviceInfo.TYPE_WIRED_HEADSET ||
                        it.type == AudioDeviceInfo.TYPE_WIRED_HEADPHONES ||
                        it.type == AudioDeviceInfo.TYPE_USB_HEADSET
                }
                AudioRoute.EARPIECE -> commDevices.firstOrNull {
                    it.type == AudioDeviceInfo.TYPE_BUILTIN_EARPIECE
                }
                AudioRoute.SPEAKER -> commDevices.firstOrNull {
                    it.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER
                }
            }
            if (targetDevice != null) {
                try {
                    audioManager.setCommunicationDevice(targetDevice)
                } catch (_: Throwable) {
                }
            }
            @Suppress("DEPRECATION")
            try {
                audioManager.isSpeakerphoneOn = (resolved == AudioRoute.SPEAKER)
            } catch (_: Throwable) {
            }
        } else {
            @Suppress("DEPRECATION")
            when (resolved) {
                AudioRoute.BLUETOOTH -> {
                    audioManager.isSpeakerphoneOn = false
                    audioManager.startBluetoothSco()
                    audioManager.isBluetoothScoOn = true
                }
                AudioRoute.WIRED_HEADSET, AudioRoute.EARPIECE -> {
                    if (audioManager.isBluetoothScoOn) {
                        audioManager.stopBluetoothSco()
                        audioManager.isBluetoothScoOn = false
                    }
                    audioManager.isSpeakerphoneOn = false
                }
                AudioRoute.SPEAKER -> {
                    if (audioManager.isBluetoothScoOn) {
                        audioManager.stopBluetoothSco()
                        audioManager.isBluetoothScoOn = false
                    }
                    audioManager.isSpeakerphoneOn = true
                }
            }
        }

        val specs = resolveDeviceSpecsForRoute(resolved)
        lastAppliedSpecs = specs
        onRouteChanged(resolved, available)
        if (notifyRust) {
            onHardwareRouteSwitched(specs.first, specs.second)
        }
    }

    private fun resolveDeviceSpecsForRoute(route: AudioRoute): Pair<String?, String?> {
        val all = audioManager.getDevices(AudioManager.GET_DEVICES_ALL)
        val inIdx = when (route) {
            AudioRoute.BLUETOOTH -> {
                findIndexedDevice(all, isSource = true) {
                    it.type == AudioDeviceInfo.TYPE_BLUETOOTH_SCO ||
                        it.type == AudioDeviceInfo.TYPE_BLE_HEADSET
                } ?: findBottomBuiltInMicIndex(all)
            }
            AudioRoute.WIRED_HEADSET -> {
                findIndexedDevice(all, isSource = true) {
                    it.type == AudioDeviceInfo.TYPE_WIRED_HEADSET ||
                        it.type == AudioDeviceInfo.TYPE_USB_HEADSET
                } ?: findBottomBuiltInMicIndex(all)
            }
            AudioRoute.SPEAKER, AudioRoute.EARPIECE -> findBottomBuiltInMicIndex(all)
        }
        val outIdx = when (route) {
            AudioRoute.SPEAKER -> findIndexedDevice(all, isSource = false) {
                it.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER
            }
            AudioRoute.EARPIECE -> findIndexedDevice(all, isSource = false) {
                it.type == AudioDeviceInfo.TYPE_BUILTIN_EARPIECE
            }
            AudioRoute.BLUETOOTH -> findIndexedDevice(all, isSource = false) {
                it.type == AudioDeviceInfo.TYPE_BLUETOOTH_SCO ||
                    it.type == AudioDeviceInfo.TYPE_BLE_HEADSET ||
                    it.type == AudioDeviceInfo.TYPE_BLUETOOTH_A2DP
            }
            AudioRoute.WIRED_HEADSET -> findIndexedDevice(all, isSource = false) {
                it.type == AudioDeviceInfo.TYPE_WIRED_HEADSET ||
                    it.type == AudioDeviceInfo.TYPE_WIRED_HEADPHONES ||
                    it.type == AudioDeviceInfo.TYPE_USB_HEADSET
            }
        }
        return inIdx?.let { "@idx:$it" } to outIdx?.let { "@idx:$it" }
    }

    private fun findBottomBuiltInMicIndex(all: Array<AudioDeviceInfo>): Int? {
        return findIndexedDevice(all, isSource = true) { dev ->
            dev.type == AudioDeviceInfo.TYPE_BUILTIN_MIC &&
                deviceAddress(dev).equals("bottom", ignoreCase = true)
        } ?: findIndexedDevice(all, isSource = true) { dev ->
            val addr = deviceAddress(dev)
            dev.type == AudioDeviceInfo.TYPE_BUILTIN_MIC &&
                !addr.equals("back", ignoreCase = true) &&
                !addr.equals("top", ignoreCase = true)
        } ?: findIndexedDevice(all, isSource = true) { dev ->
            dev.type == AudioDeviceInfo.TYPE_BUILTIN_MIC
        }
    }

    private fun deviceAddress(dev: AudioDeviceInfo): String {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            dev.address
        } else {
            ""
        }
    }

    private inline fun findIndexedDevice(
        all: Array<AudioDeviceInfo>,
        isSource: Boolean,
        predicate: (AudioDeviceInfo) -> Boolean,
    ): Int? {
        for (idx in all.indices) {
            val dev = all[idx]
            val matchesDir = if (isSource) dev.isSource else dev.isSink
            if (matchesDir && predicate(dev)) {
                return idx
            }
        }
        return null
    }

    private fun computeAvailableRoutes(): List<AudioRoute> {
        val routes = mutableListOf(AudioRoute.SPEAKER)
        val devices = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            audioManager.availableCommunicationDevices
        } else {
            audioManager.getDevices(AudioManager.GET_DEVICES_OUTPUTS).toList()
        }
        val hasEarpiece = devices.any { it.type == AudioDeviceInfo.TYPE_BUILTIN_EARPIECE }
        val hasBluetooth = devices.any {
            it.type == AudioDeviceInfo.TYPE_BLUETOOTH_SCO ||
                it.type == AudioDeviceInfo.TYPE_BLUETOOTH_A2DP ||
                it.type == AudioDeviceInfo.TYPE_BLE_HEADSET
        }
        val hasWired = devices.any {
            it.type == AudioDeviceInfo.TYPE_WIRED_HEADSET ||
                it.type == AudioDeviceInfo.TYPE_WIRED_HEADPHONES ||
                it.type == AudioDeviceInfo.TYPE_USB_HEADSET
        }
        if (hasEarpiece) routes.add(AudioRoute.EARPIECE)
        if (hasBluetooth) routes.add(AudioRoute.BLUETOOTH)
        if (hasWired) routes.add(AudioRoute.WIRED_HEADSET)
        return routes
    }
}
