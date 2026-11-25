package app.squadair.mobile.service

enum class AudioRoute(val label: String) {
    SPEAKER("Speaker"),
    EARPIECE("Earpiece"),
    BLUETOOTH("Bluetooth"),
    WIRED_HEADSET("Headset"),
}

data class AudioDeviceSnapshot(
    val id: Int,
    val type: Int,
    val isSource: Boolean,
    val isSink: Boolean,
    val address: String = "",
)

object AudioRoutePlanner {
    const val TYPE_BUILTIN_EARPIECE = 1
    const val TYPE_BUILTIN_SPEAKER = 2
    const val TYPE_WIRED_HEADSET = 3
    const val TYPE_WIRED_HEADPHONES = 4
    const val TYPE_BLUETOOTH_SCO = 7
    const val TYPE_BLUETOOTH_A2DP = 8
    const val TYPE_USB_DEVICE = 11
    const val TYPE_BUILTIN_MIC = 15
    const val TYPE_USB_HEADSET = 22
    const val TYPE_BLE_HEADSET = 26

    fun computeAvailableRoutes(
        devices: List<AudioDeviceSnapshot>,
        isApi31Plus: Boolean,
    ): List<AudioRoute> {
        val routes = mutableListOf(AudioRoute.SPEAKER)
        val hasEarpiece = devices.any { it.type == TYPE_BUILTIN_EARPIECE }
        val hasBluetooth = if (isApi31Plus) {
            devices.any {
                it.type == TYPE_BLE_HEADSET || it.type == TYPE_BLUETOOTH_SCO
            }
        } else {
            devices.any {
                it.type == TYPE_BLUETOOTH_SCO ||
                    it.type == TYPE_BLE_HEADSET ||
                    it.type == TYPE_BLUETOOTH_A2DP
            }
        }
        val hasWired = devices.any {
            it.type == TYPE_WIRED_HEADSET ||
                it.type == TYPE_WIRED_HEADPHONES ||
                it.type == TYPE_USB_HEADSET ||
                (it.type == TYPE_USB_DEVICE && it.isSink)
        }
        if (hasEarpiece) routes.add(AudioRoute.EARPIECE)
        if (hasBluetooth) routes.add(AudioRoute.BLUETOOTH)
        if (hasWired) routes.add(AudioRoute.WIRED_HEADSET)
        return routes
    }

    fun pickInitialRoute(available: List<AudioRoute>): AudioRoute {
        return when {
            available.contains(AudioRoute.BLUETOOTH) -> AudioRoute.BLUETOOTH
            available.contains(AudioRoute.WIRED_HEADSET) -> AudioRoute.WIRED_HEADSET
            available.contains(AudioRoute.SPEAKER) -> AudioRoute.SPEAKER
            else -> available.firstOrNull() ?: AudioRoute.SPEAKER
        }
    }

    fun isBluetoothRouteDevice(
        device: AudioDeviceSnapshot,
        isApi31Plus: Boolean,
    ): Boolean {
        return if (isApi31Plus) {
            device.type == TYPE_BLE_HEADSET || device.type == TYPE_BLUETOOTH_SCO
        } else {
            device.type == TYPE_BLUETOOTH_SCO ||
                device.type == TYPE_BLE_HEADSET ||
                device.type == TYPE_BLUETOOTH_A2DP
        }
    }

    fun isWiredRouteDevice(device: AudioDeviceSnapshot): Boolean {
        return device.type == TYPE_WIRED_HEADSET ||
            device.type == TYPE_WIRED_HEADPHONES ||
            device.type == TYPE_USB_HEADSET ||
            (device.type == TYPE_USB_DEVICE && device.isSink)
    }

    fun reconcileRouteOnHardwareChange(
        currentRoute: AudioRoute,
        available: List<AudioRoute>,
        added: Boolean,
        previousAvailable: List<AudioRoute>? = null,
        addedDevices: List<AudioDeviceSnapshot> = emptyList(),
        isApi31Plus: Boolean = true,
    ): AudioRoute {
        val btMatchesAdded = if (addedDevices.isNotEmpty()) {
            addedDevices.any { isBluetoothRouteDevice(it, isApi31Plus) }
        } else {
            true
        }
        val wiredMatchesAdded = if (addedDevices.isNotEmpty()) {
            addedDevices.any { isWiredRouteDevice(it) }
        } else {
            true
        }
        val btNewlyAdded = added &&
            available.contains(AudioRoute.BLUETOOTH) &&
            (previousAvailable == null || !previousAvailable.contains(AudioRoute.BLUETOOTH)) &&
            btMatchesAdded &&
            (previousAvailable != null || addedDevices.isNotEmpty())
        val wiredNewlyAdded = added &&
            available.contains(AudioRoute.WIRED_HEADSET) &&
            (previousAvailable == null || !previousAvailable.contains(AudioRoute.WIRED_HEADSET)) &&
            wiredMatchesAdded &&
            (previousAvailable != null || addedDevices.isNotEmpty())
        return when {
            btNewlyAdded -> AudioRoute.BLUETOOTH
            wiredNewlyAdded -> AudioRoute.WIRED_HEADSET
            !available.contains(currentRoute) -> pickInitialRoute(available)
            else -> currentRoute
        }
    }

    fun selectCommunicationDeviceId(
        route: AudioRoute,
        commDevices: List<AudioDeviceSnapshot>,
    ): Int? {
        return when (route) {
            AudioRoute.BLUETOOTH -> {
                commDevices.firstOrNull { it.type == TYPE_BLE_HEADSET }?.id
                    ?: commDevices.firstOrNull { it.type == TYPE_BLUETOOTH_SCO }?.id
            }
            AudioRoute.WIRED_HEADSET -> {
                commDevices.firstOrNull { it.type == TYPE_USB_HEADSET }?.id
                    ?: commDevices.firstOrNull { it.type == TYPE_WIRED_HEADSET }?.id
                    ?: commDevices.firstOrNull { it.type == TYPE_WIRED_HEADPHONES }?.id
                    ?: commDevices.firstOrNull { it.type == TYPE_USB_DEVICE && it.isSink }?.id
            }
            AudioRoute.EARPIECE -> commDevices.firstOrNull { it.type == TYPE_BUILTIN_EARPIECE }?.id
            AudioRoute.SPEAKER -> commDevices.firstOrNull { it.type == TYPE_BUILTIN_SPEAKER }?.id
        }
    }

    fun mapCommunicationDeviceTypeToRoute(deviceType: Int?): AudioRoute? {
        return when (deviceType) {
            TYPE_BLE_HEADSET, TYPE_BLUETOOTH_SCO -> AudioRoute.BLUETOOTH
            TYPE_USB_HEADSET, TYPE_WIRED_HEADSET, TYPE_WIRED_HEADPHONES, TYPE_USB_DEVICE -> {
                AudioRoute.WIRED_HEADSET
            }
            TYPE_BUILTIN_EARPIECE -> AudioRoute.EARPIECE
            TYPE_BUILTIN_SPEAKER -> AudioRoute.SPEAKER
            else -> null
        }
    }

    fun findBottomBuiltInMicId(inputDevices: List<AudioDeviceSnapshot>): Int? {
        val mics = inputDevices.filter { it.isSource && it.type == TYPE_BUILTIN_MIC }
        if (mics.isEmpty()) return null
        return mics.firstOrNull { it.address.equals("bottom", ignoreCase = true) }?.id
            ?: mics.firstOrNull { dev ->
                !dev.address.equals("back", ignoreCase = true) &&
                    !dev.address.equals("top", ignoreCase = true) &&
                    !dev.address.equals("both", ignoreCase = true)
            }?.id
            ?: mics.firstOrNull()?.id
    }

    fun resolveInputSpec(
        route: AudioRoute,
        inputDevices: List<AudioDeviceSnapshot>,
        activeCommDeviceType: Int? = null,
    ): String? {
        val selectedId = when (route) {
            AudioRoute.SPEAKER, AudioRoute.EARPIECE -> findBottomBuiltInMicId(inputDevices)
            AudioRoute.WIRED_HEADSET -> {
                inputDevices.firstOrNull {
                    it.isSource && (it.type == TYPE_USB_HEADSET || it.type == TYPE_WIRED_HEADSET)
                }?.id
                    ?: inputDevices.firstOrNull { it.isSource && it.type == TYPE_USB_DEVICE }?.id
                    ?: findBottomBuiltInMicId(inputDevices)
            }
            AudioRoute.BLUETOOTH -> {
                val preferBle = activeCommDeviceType == TYPE_BLE_HEADSET
                val btId = if (preferBle) {
                    inputDevices.firstOrNull { it.isSource && it.type == TYPE_BLE_HEADSET }?.id
                        ?: inputDevices.firstOrNull { it.isSource && it.type == TYPE_BLUETOOTH_SCO }?.id
                } else {
                    inputDevices.firstOrNull { it.isSource && it.type == TYPE_BLUETOOTH_SCO }?.id
                        ?: inputDevices.firstOrNull { it.isSource && it.type == TYPE_BLE_HEADSET }?.id
                }
                btId ?: findBottomBuiltInMicId(inputDevices)
            }
        }
        return selectedId?.let { "@id:$it" }
    }

    fun resolveOutputSpec(@Suppress("UNUSED_PARAMETER") route: AudioRoute): String? = null
}
