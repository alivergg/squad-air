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
    private val onHardwareRouteSwitched: () -> Unit,
) {
    private val audioManager = context.getSystemService(Context.AUDIO_SERVICE) as AudioManager
    private val mainHandler = Handler(Looper.getMainLooper())
    private var focusRequest: AudioFocusRequest? = null
    private var active = false
    private var currentRoute: AudioRoute = AudioRoute.SPEAKER

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
        applyRouteInternal(initial, notifyRust = false)
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
            onHardwareRouteSwitched()
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

        onRouteChanged(resolved, available)
        if (notifyRust) {
            onHardwareRouteSwitched()
        }
    }

    private fun computeAvailableRoutes(): List<AudioRoute> {
        val routes = mutableListOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE)
        val devices = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            audioManager.availableCommunicationDevices
        } else {
            audioManager.getDevices(AudioManager.GET_DEVICES_OUTPUTS).toList()
        }
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
        if (hasBluetooth) routes.add(AudioRoute.BLUETOOTH)
        if (hasWired) routes.add(AudioRoute.WIRED_HEADSET)
        return routes
    }
}
