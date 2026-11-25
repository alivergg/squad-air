package app.squadair.mobile.service

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AudioRoutePlannerTest {
    private val earpiece = AudioDeviceSnapshot(
        id = 1,
        type = AudioRoutePlanner.TYPE_BUILTIN_EARPIECE,
        isSource = false,
        isSink = true,
    )
    private val speaker = AudioDeviceSnapshot(
        id = 2,
        type = AudioRoutePlanner.TYPE_BUILTIN_SPEAKER,
        isSource = false,
        isSink = true,
    )
    private val wiredHeadsetOut = AudioDeviceSnapshot(
        id = 3,
        type = AudioRoutePlanner.TYPE_WIRED_HEADSET,
        isSource = false,
        isSink = true,
    )
    private val wiredHeadphonesOut = AudioDeviceSnapshot(
        id = 4,
        type = AudioRoutePlanner.TYPE_WIRED_HEADPHONES,
        isSource = false,
        isSink = true,
    )
    private val usbHeadsetOut = AudioDeviceSnapshot(
        id = 5,
        type = AudioRoutePlanner.TYPE_USB_HEADSET,
        isSource = false,
        isSink = true,
    )
    private val usbDongleOut = AudioDeviceSnapshot(
        id = 6,
        type = AudioRoutePlanner.TYPE_USB_DEVICE,
        isSource = false,
        isSink = true,
    )
    private val btScoOut = AudioDeviceSnapshot(
        id = 7,
        type = AudioRoutePlanner.TYPE_BLUETOOTH_SCO,
        isSource = false,
        isSink = true,
    )
    private val btA2dpOut = AudioDeviceSnapshot(
        id = 8,
        type = AudioRoutePlanner.TYPE_BLUETOOTH_A2DP,
        isSource = false,
        isSink = true,
    )
    private val bleHeadsetOut = AudioDeviceSnapshot(
        id = 9,
        type = AudioRoutePlanner.TYPE_BLE_HEADSET,
        isSource = false,
        isSink = true,
    )

    private val s10BottomMic = AudioDeviceSnapshot(
        id = 17,
        type = AudioRoutePlanner.TYPE_BUILTIN_MIC,
        isSource = true,
        isSink = false,
        address = "bottom",
    )
    private val s10BackMic = AudioDeviceSnapshot(
        id = 19,
        type = AudioRoutePlanner.TYPE_BUILTIN_MIC,
        isSource = true,
        isSink = false,
        address = "back",
    )
    private val s10BothMic = AudioDeviceSnapshot(
        id = 20,
        type = AudioRoutePlanner.TYPE_BUILTIN_MIC,
        isSource = true,
        isSink = false,
        address = "both",
    )

    @Test
    fun computeAvailableRoutes_api31ExcludesA2dpOnlyBluetooth() {
        val routes = AudioRoutePlanner.computeAvailableRoutes(
            devices = listOf(earpiece, speaker, btA2dpOut),
            isApi31Plus = true,
        )
        assertEquals(listOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE), routes)
        assertFalse(routes.contains(AudioRoute.BLUETOOTH))
    }

    @Test
    fun computeAvailableRoutes_api31IncludesScoAndBleAndWiredTypes() {
        val scoRoutes = AudioRoutePlanner.computeAvailableRoutes(
            devices = listOf(earpiece, speaker, btScoOut, wiredHeadphonesOut),
            isApi31Plus = true,
        )
        assertEquals(
            listOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE, AudioRoute.BLUETOOTH, AudioRoute.WIRED_HEADSET),
            scoRoutes,
        )

        val bleAndUsbRoutes = AudioRoutePlanner.computeAvailableRoutes(
            devices = listOf(speaker, bleHeadsetOut, usbHeadsetOut),
            isApi31Plus = true,
        )
        assertTrue(bleAndUsbRoutes.contains(AudioRoute.BLUETOOTH))
        assertTrue(bleAndUsbRoutes.contains(AudioRoute.WIRED_HEADSET))

        val usbDongleRoutes = AudioRoutePlanner.computeAvailableRoutes(
            devices = listOf(speaker, usbDongleOut),
            isApi31Plus = true,
        )
        assertTrue(usbDongleRoutes.contains(AudioRoute.WIRED_HEADSET))
    }

    @Test
    fun computeAvailableRoutes_legacyAllowsA2dpForScoFallback() {
        val routes = AudioRoutePlanner.computeAvailableRoutes(
            devices = listOf(earpiece, speaker, btA2dpOut),
            isApi31Plus = false,
        )
        assertTrue(routes.contains(AudioRoute.BLUETOOTH))
    }

    @Test
    fun pickInitialRoute_prefersBluetoothThenWiredThenSpeaker() {
        assertEquals(
            AudioRoute.BLUETOOTH,
            AudioRoutePlanner.pickInitialRoute(
                listOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE, AudioRoute.WIRED_HEADSET, AudioRoute.BLUETOOTH),
            ),
        )
        assertEquals(
            AudioRoute.WIRED_HEADSET,
            AudioRoutePlanner.pickInitialRoute(
                listOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE, AudioRoute.WIRED_HEADSET),
            ),
        )
        assertEquals(
            AudioRoute.SPEAKER,
            AudioRoutePlanner.pickInitialRoute(listOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE)),
        )
    }

    @Test
    fun reconcileRouteOnHardwareChange_switchesAndFallsBackCorrectly() {
        assertEquals(
            AudioRoute.WIRED_HEADSET,
            AudioRoutePlanner.reconcileRouteOnHardwareChange(
                currentRoute = AudioRoute.SPEAKER,
                available = listOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE, AudioRoute.WIRED_HEADSET),
                added = true,
                previousAvailable = listOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE),
                addedDevices = listOf(wiredHeadsetOut),
            ),
        )
        assertEquals(
            AudioRoute.EARPIECE,
            AudioRoutePlanner.reconcileRouteOnHardwareChange(
                currentRoute = AudioRoute.EARPIECE,
                available = listOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE, AudioRoute.WIRED_HEADSET),
                added = true,
                previousAvailable = listOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE, AudioRoute.WIRED_HEADSET),
                addedDevices = listOf(s10BottomMic),
            ),
        )
        assertEquals(
            AudioRoute.BLUETOOTH,
            AudioRoutePlanner.reconcileRouteOnHardwareChange(
                currentRoute = AudioRoute.EARPIECE,
                available = listOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE, AudioRoute.BLUETOOTH),
                added = true,
                previousAvailable = listOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE),
                addedDevices = listOf(btScoOut),
            ),
        )
        assertEquals(
            AudioRoute.WIRED_HEADSET,
            AudioRoutePlanner.reconcileRouteOnHardwareChange(
                currentRoute = AudioRoute.BLUETOOTH,
                available = listOf(AudioRoute.SPEAKER, AudioRoute.WIRED_HEADSET),
                added = false,
            ),
        )
        assertEquals(
            AudioRoute.SPEAKER,
            AudioRoutePlanner.reconcileRouteOnHardwareChange(
                currentRoute = AudioRoute.WIRED_HEADSET,
                available = listOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE),
                added = false,
            ),
        )
    }

    @Test
    fun reconcileRouteOnHardwareChange_alreadyConnectedBtDoesNotHijackManualSpeakerOrEarpiece() {
        val prevWithBt = listOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE, AudioRoute.BLUETOOTH)
        assertEquals(
            AudioRoute.SPEAKER,
            AudioRoutePlanner.reconcileRouteOnHardwareChange(
                currentRoute = AudioRoute.SPEAKER,
                available = prevWithBt,
                added = true,
                previousAvailable = prevWithBt,
                addedDevices = listOf(s10BottomMic),
            ),
        )
        assertEquals(
            AudioRoute.EARPIECE,
            AudioRoutePlanner.reconcileRouteOnHardwareChange(
                currentRoute = AudioRoute.EARPIECE,
                available = prevWithBt,
                added = true,
                previousAvailable = prevWithBt,
                addedDevices = listOf(btA2dpOut),
                isApi31Plus = true,
            ),
        )
        assertEquals(
            AudioRoute.SPEAKER,
            AudioRoutePlanner.reconcileRouteOnHardwareChange(
                currentRoute = AudioRoute.SPEAKER,
                available = prevWithBt,
                added = true,
                previousAvailable = prevWithBt,
                addedDevices = listOf(btScoOut),
            ),
        )
    }

    @Test
    fun reconcileRouteOnHardwareChange_btConnectedAndManualSpeakerThenWiredAddedSwitchesToWired() {
        val prevWithBt = listOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE, AudioRoute.BLUETOOTH)
        val availWithBtAndWired = listOf(
            AudioRoute.SPEAKER,
            AudioRoute.EARPIECE,
            AudioRoute.BLUETOOTH,
            AudioRoute.WIRED_HEADSET,
        )
        assertEquals(
            AudioRoute.WIRED_HEADSET,
            AudioRoutePlanner.reconcileRouteOnHardwareChange(
                currentRoute = AudioRoute.SPEAKER,
                available = availWithBtAndWired,
                added = true,
                previousAvailable = prevWithBt,
                addedDevices = listOf(wiredHeadsetOut),
            ),
        )
        assertEquals(
            AudioRoute.WIRED_HEADSET,
            AudioRoutePlanner.reconcileRouteOnHardwareChange(
                currentRoute = AudioRoute.EARPIECE,
                available = availWithBtAndWired,
                added = true,
                previousAvailable = prevWithBt,
                addedDevices = listOf(usbHeadsetOut),
            ),
        )
        assertEquals(
            AudioRoute.WIRED_HEADSET,
            AudioRoutePlanner.reconcileRouteOnHardwareChange(
                currentRoute = AudioRoute.SPEAKER,
                available = availWithBtAndWired,
                added = true,
                previousAvailable = null,
                addedDevices = listOf(usbDongleOut),
            ),
        )
    }

    @Test
    fun reconcileRouteOnHardwareChange_wiredConnectedThenBtNewlyAddedSwitchesToBluetooth() {
        val prevWithWired = listOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE, AudioRoute.WIRED_HEADSET)
        val availWithBoth = listOf(
            AudioRoute.SPEAKER,
            AudioRoute.EARPIECE,
            AudioRoute.BLUETOOTH,
            AudioRoute.WIRED_HEADSET,
        )
        assertEquals(
            AudioRoute.BLUETOOTH,
            AudioRoutePlanner.reconcileRouteOnHardwareChange(
                currentRoute = AudioRoute.WIRED_HEADSET,
                available = availWithBoth,
                added = true,
                previousAvailable = prevWithWired,
                addedDevices = listOf(bleHeadsetOut),
                isApi31Plus = true,
            ),
        )
        assertEquals(
            AudioRoute.BLUETOOTH,
            AudioRoutePlanner.reconcileRouteOnHardwareChange(
                currentRoute = AudioRoute.WIRED_HEADSET,
                available = availWithBoth,
                added = true,
                previousAvailable = prevWithWired,
                addedDevices = listOf(btScoOut),
                isApi31Plus = true,
            ),
        )
    }

    @Test
    fun reconcileRouteOnHardwareChange_unpluggingActiveOrInactiveDevicesFallsBackCleanly() {
        assertEquals(
            AudioRoute.WIRED_HEADSET,
            AudioRoutePlanner.reconcileRouteOnHardwareChange(
                currentRoute = AudioRoute.BLUETOOTH,
                available = listOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE, AudioRoute.WIRED_HEADSET),
                added = false,
                previousAvailable = listOf(
                    AudioRoute.SPEAKER,
                    AudioRoute.EARPIECE,
                    AudioRoute.BLUETOOTH,
                    AudioRoute.WIRED_HEADSET,
                ),
            ),
        )
        assertEquals(
            AudioRoute.BLUETOOTH,
            AudioRoutePlanner.reconcileRouteOnHardwareChange(
                currentRoute = AudioRoute.WIRED_HEADSET,
                available = listOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE, AudioRoute.BLUETOOTH),
                added = false,
                previousAvailable = listOf(
                    AudioRoute.SPEAKER,
                    AudioRoute.EARPIECE,
                    AudioRoute.BLUETOOTH,
                    AudioRoute.WIRED_HEADSET,
                ),
            ),
        )
        assertEquals(
            AudioRoute.SPEAKER,
            AudioRoutePlanner.reconcileRouteOnHardwareChange(
                currentRoute = AudioRoute.SPEAKER,
                available = listOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE),
                added = false,
                previousAvailable = listOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE, AudioRoute.BLUETOOTH),
            ),
        )
    }

    @Test
    fun selectCommunicationDeviceId_prefersExpectedDevices() {
        val commDevices = listOf(earpiece, speaker, wiredHeadphonesOut, wiredHeadsetOut, usbHeadsetOut, btScoOut, bleHeadsetOut)
        assertEquals(9, AudioRoutePlanner.selectCommunicationDeviceId(AudioRoute.BLUETOOTH, commDevices))
        assertEquals(5, AudioRoutePlanner.selectCommunicationDeviceId(AudioRoute.WIRED_HEADSET, commDevices))
        assertEquals(1, AudioRoutePlanner.selectCommunicationDeviceId(AudioRoute.EARPIECE, commDevices))
        assertEquals(2, AudioRoutePlanner.selectCommunicationDeviceId(AudioRoute.SPEAKER, commDevices))
    }

    @Test
    fun resolveInputSpec_selectsBottomMicForEarpieceAndSpeakerOnS10Layout() {
        val inputs = listOf(s10BackMic, s10BothMic, s10BottomMic)
        assertEquals("@id:17", AudioRoutePlanner.resolveInputSpec(AudioRoute.EARPIECE, inputs))
        assertEquals("@id:17", AudioRoutePlanner.resolveInputSpec(AudioRoute.SPEAKER, inputs))
    }

    @Test
    fun resolveInputSpec_fallbackSkipsBackTopBothWhenBottomLabelAbsent() {
        val backMic = AudioDeviceSnapshot(10, AudioRoutePlanner.TYPE_BUILTIN_MIC, true, false, "back")
        val topMic = AudioDeviceSnapshot(11, AudioRoutePlanner.TYPE_BUILTIN_MIC, true, false, "top")
        val bothMic = AudioDeviceSnapshot(12, AudioRoutePlanner.TYPE_BUILTIN_MIC, true, false, "both")
        val unlabeledBottom = AudioDeviceSnapshot(13, AudioRoutePlanner.TYPE_BUILTIN_MIC, true, false, "")
        assertEquals(
            "@id:13",
            AudioRoutePlanner.resolveInputSpec(AudioRoute.EARPIECE, listOf(backMic, topMic, bothMic, unlabeledBottom)),
        )
    }

    @Test
    fun resolveInputSpec_wiredHeadsetUsesHeadsetMicOrFallsBackToBottomMic() {
        val wiredMic = AudioDeviceSnapshot(31, AudioRoutePlanner.TYPE_WIRED_HEADSET, true, false)
        val usbMic = AudioDeviceSnapshot(32, AudioRoutePlanner.TYPE_USB_HEADSET, true, false)
        assertEquals(
            "@id:31",
            AudioRoutePlanner.resolveInputSpec(AudioRoute.WIRED_HEADSET, listOf(s10BottomMic, wiredMic)),
        )
        assertEquals(
            "@id:32",
            AudioRoutePlanner.resolveInputSpec(AudioRoute.WIRED_HEADSET, listOf(s10BottomMic, usbMic)),
        )
        assertEquals(
            "@id:17",
            AudioRoutePlanner.resolveInputSpec(AudioRoute.WIRED_HEADSET, listOf(s10BottomMic, s10BackMic)),
        )
    }

    @Test
    fun resolveInputSpec_bluetoothUsesMatchingBtMicOrFallsBackToBottomMic() {
        val scoMic = AudioDeviceSnapshot(41, AudioRoutePlanner.TYPE_BLUETOOTH_SCO, true, false)
        val bleMic = AudioDeviceSnapshot(42, AudioRoutePlanner.TYPE_BLE_HEADSET, true, false)
        assertEquals(
            "@id:41",
            AudioRoutePlanner.resolveInputSpec(
                AudioRoute.BLUETOOTH,
                listOf(s10BottomMic, scoMic, bleMic),
                activeCommDeviceType = AudioRoutePlanner.TYPE_BLUETOOTH_SCO,
            ),
        )
        assertEquals(
            "@id:42",
            AudioRoutePlanner.resolveInputSpec(
                AudioRoute.BLUETOOTH,
                listOf(s10BottomMic, scoMic, bleMic),
                activeCommDeviceType = AudioRoutePlanner.TYPE_BLE_HEADSET,
            ),
        )
        assertEquals(
            "@id:17",
            AudioRoutePlanner.resolveInputSpec(AudioRoute.BLUETOOTH, listOf(s10BottomMic)),
        )
    }

    @Test
    fun resolveOutputSpec_alwaysReturnsNull() {
        for (route in AudioRoute.entries) {
            assertNull(AudioRoutePlanner.resolveOutputSpec(route))
        }
    }
}
