package app.squadair.mobile.core

import android.content.Context
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Environment
import app.squadair.mobile.service.AudioRoute
import app.squadair.mobile.service.AudioRouteController
import app.squadair.mobile.service.VoiceCallForegroundService
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import uniffi.squad_core.MobileAudioTelemetry
import uniffi.squad_core.MobileBlobTransferState
import uniffi.squad_core.MobileChatContent
import uniffi.squad_core.MobileChatItem
import uniffi.squad_core.MobileCoreController
import uniffi.squad_core.MobileCoreException
import uniffi.squad_core.MobileEventListener
import uniffi.squad_core.MobileFriendItem
import uniffi.squad_core.MobileIncomingCall
import uniffi.squad_core.MobileLanRoomItem
import uniffi.squad_core.MobilePeerSnapshot
import uniffi.squad_core.MobileProfileSnapshot
import uniffi.squad_core.MobileRemoteStreamInfo
import uniffi.squad_core.MobileSavedChannel
import java.io.File
import java.net.Inet4Address
import java.util.concurrent.atomic.AtomicBoolean

data class SquadUiState(
    val profile: MobileProfileSnapshot = MobileProfileSnapshot(
        nickname = "SquadUser",
        clientUuidHex = "",
        myFriendCode = "",
        rendezvousServer = "rv.squadair.app",
        customRendezvousServer = "",
        micMuted = false,
        deafened = false,
        aecEnabled = false,
        nsMode = "Rnnoise",
        vadThresholdDb = -45f,
        inputGain = 1.0f,
        streamVolume = 1.0f,
    ),
    val friends: List<MobileFriendItem> = emptyList(),
    val savedChannels: List<MobileSavedChannel> = emptyList(),
    val lanRooms: List<MobileLanRoomItem> = emptyList(),
    val inRoom: Boolean = false,
    val localPeerId: UShort = 0u,
    val activeInviteUri: String = "",
    val isLanLocked: Boolean = false,
    val peers: List<MobilePeerSnapshot> = emptyList(),
    val watchedPeerId: UShort? = null,
    val chatMessages: List<MobileChatItem> = emptyList(),
    val unreadChatCount: Int = 0,
    val isChatOpen: Boolean = false,
    val peerAvatarsRgba: Map<String, ByteArray> = emptyMap(),
    val activeShortCode: String? = null,
    val shortCodeSecondsRemaining: Int = 0,
    val incomingCall: MobileIncomingCall? = null,
    val invitedFriendsSession: Set<String> = emptySet(),
    val lanAuthError: String? = null,
    val bannerMessage: String? = null,
    val audioRoute: AudioRoute = AudioRoute.SPEAKER,
    val availableAudioRoutes: List<AudioRoute> = listOf(AudioRoute.SPEAKER, AudioRoute.EARPIECE),
    val isFullscreenStream: Boolean = false,
    val isInPipMode: Boolean = false,
)

class SquadSessionRepository private constructor(private val appContext: Context) : MobileEventListener {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private lateinit var controller: MobileCoreController
    private val initialized = AtomicBoolean(false)

    private val _uiState = MutableStateFlow(SquadUiState())
    val uiState: StateFlow<SquadUiState> = _uiState.asStateFlow()

    private val _telemetry = MutableStateFlow(
        MobileAudioTelemetry(
            localMicLevelDb = -60f,
            isLocalSpeaking = false,
            jitterTargetMs = 40u,
        ),
    )
    val telemetry: StateFlow<MobileAudioTelemetry> = _telemetry.asStateFlow()

    private val audioRouteController = AudioRouteController(
        context = appContext,
        onRouteChanged = { active, available ->
            _uiState.update {
                it.copy(
                    audioRoute = active,
                    availableAudioRoutes = available,
                )
            }
        },
        onHardwareRouteSwitched = {
            if (initialized.get()) {
                controller.notifyAndroidAudioRouteChanged()
            }
        },
    )

    private var telemetryJob: Job? = null
    private var shortCodeTimerJob: Job? = null
    private var networkCallbackRegistered = false

    private fun startIfNeeded() {
        if (!initialized.compareAndSet(false, true)) {
            return
        }
        SquadNativeBridge.ensureInitialized(appContext)

        val filesDir = File(appContext.filesDir, "squad-air").apply { mkdirs() }
        val cacheDir = File(appContext.cacheDir, "squad-air").apply { mkdirs() }
        val downloadsDir = File(
            appContext.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS) ?: appContext.filesDir,
            "SquadAir",
        ).apply { mkdirs() }

        controller = MobileCoreController(
            filesDir = filesDir.absolutePath,
            cacheDir = cacheDir.absolutePath,
            downloadsDir = downloadsDir.absolutePath,
            eventListener = this,
        )

        refreshStaticState()
        registerConnectivityWatcher()
        pushCurrentLanIpv4s()
    }

    private fun refreshStaticState() {
        val profile = controller.getProfileSnapshot()
        val friends = controller.getFriends()
        val saved = controller.getSavedChannels()
        val selfAvatar = controller.getIdenticonRgba(profile.clientUuidHex)
        _uiState.update {
            it.copy(
                profile = profile,
                friends = friends,
                savedChannels = saved,
                peerAvatarsRgba = it.peerAvatarsRgba + (profile.clientUuidHex to selfAvatar),
            )
        }
    }

    fun getIdenticonForUuid(clientUuidHex: String): ByteArray {
        val existing = _uiState.value.peerAvatarsRgba[clientUuidHex]
        if (existing != null) return existing
        return controller.getIdenticonRgba(clientUuidHex)
    }

    fun getFriendIdenticon(pubKeyHex: String): ByteArray {
        return controller.getFriendIdenticonRgba(pubKeyHex)
    }

    fun createAndJoinRoom() {
        runAction {
            clearRoomDrafts()
            controller.createAndJoinRoom()
        }
    }

    fun joinRoomByInvite(inviteUri: String) {
        val trimmed = inviteUri.trim()
        if (trimmed.isEmpty()) return
        runAction {
            clearRoomDrafts()
            controller.joinRoomByInvite(trimmed)
        }
    }

    fun joinRoomByShortCode(code6Digits: String) {
        runAction {
            clearRoomDrafts()
            controller.joinRoomByShortCode(code6Digits)
        }
    }

    fun joinLanRoom(roomIdHex: String, password: String?) {
        runAction {
            _uiState.update { it.copy(lanAuthError = null) }
            controller.joinLanRoom(roomIdHex, password)
        }
    }

    fun clearLanAuthError() {
        _uiState.update { it.copy(lanAuthError = null) }
    }

    fun leaveRoom() {
        scope.launch {
            controller.leaveRoom()
        }
        onLeftRoomInternal()
    }

    fun generateShortCode() {
        scope.launch {
            controller.generateShortCode()
        }
    }

    fun toggleMicMute(): Boolean {
        val next = controller.toggleMicMute()
        _uiState.update { it.copy(profile = it.profile.copy(micMuted = next)) }
        VoiceCallForegroundService.updateNotificationIfRunning(appContext)
        return next
    }

    fun toggleDeafen(): Boolean {
        val next = controller.toggleDeafen()
        _uiState.update { it.copy(profile = it.profile.copy(deafened = next)) }
        VoiceCallForegroundService.updateNotificationIfRunning(appContext)
        return next
    }

    fun setAudioRoute(route: AudioRoute) {
        audioRouteController.selectRoute(route)
    }

    fun setAecEnabled(enabled: Boolean) {
        controller.setAecEnabled(enabled)
        _uiState.update { it.copy(profile = it.profile.copy(aecEnabled = enabled)) }
    }

    fun setRnnoiseEnabled(enabled: Boolean) {
        controller.setRnnoiseEnabled(enabled)
        val label = if (enabled) "Rnnoise" else "Off"
        _uiState.update { it.copy(profile = it.profile.copy(nsMode = label)) }
    }

    fun setInputGain(gain: Float) {
        controller.setInputGain(gain)
        _uiState.update { it.copy(profile = it.profile.copy(inputGain = gain)) }
    }

    fun setVadThresholdDb(db: Float) {
        controller.setVadThresholdDb(db)
        _uiState.update { it.copy(profile = it.profile.copy(vadThresholdDb = db)) }
    }

    fun setPeerVolume(clientUuidHex: String, gain: Float) {
        runAction {
            controller.setPeerVolume(clientUuidHex, gain)
            _uiState.update { st ->
                st.copy(
                    peers = st.peers.map { p ->
                        if (p.clientUuidHex == clientUuidHex) p.copy(volumeGain = gain) else p
                    },
                )
            }
        }
    }

    fun setStreamVolume(gain: Float) {
        controller.setStreamVolume(gain)
        _uiState.update { it.copy(profile = it.profile.copy(streamVolume = gain)) }
    }

    fun setNickname(nickname: String) {
        runAction {
            controller.setNickname(nickname)
            refreshStaticState()
        }
    }

    fun setRendezvousServer(server: String) {
        runAction {
            controller.setRendezvousServer(server)
            refreshStaticState()
        }
    }

    fun addFriendByCode(code: String) {
        runAction {
            controller.addFriendByCode(code)
            _uiState.update { it.copy(friends = controller.getFriends()) }
        }
    }

    fun removeFriend(pubKeyHex: String) {
        runAction {
            controller.removeFriend(pubKeyHex)
            _uiState.update { it.copy(friends = controller.getFriends()) }
        }
    }

    fun callOrInviteFriend(pubKeyHex: String) {
        runAction {
            controller.callOrInviteFriend(pubKeyHex)
        }
    }

    fun acceptIncomingCall(inviteUri: String) {
        runAction {
            _uiState.update { it.copy(incomingCall = null) }
            clearRoomDrafts()
            controller.acceptIncomingCall(inviteUri)
        }
    }

    fun declineIncomingCall() {
        controller.declineIncomingCall()
        _uiState.update { it.copy(incomingCall = null) }
    }

    fun saveChannel(name: String, inviteUri: String) {
        runAction {
            controller.saveChannel(name, inviteUri)
            _uiState.update { it.copy(savedChannels = controller.getSavedChannels()) }
        }
    }

    fun removeSavedChannel(inviteUri: String) {
        controller.removeSavedChannel(inviteUri)
        _uiState.update { it.copy(savedChannels = controller.getSavedChannels()) }
    }

    fun watchPeerStream(peerId: UShort?) {
        _uiState.update {
            it.copy(
                watchedPeerId = peerId,
                isFullscreenStream = if (peerId == null) false else it.isFullscreenStream,
            )
        }
        scope.launch {
            controller.watchPeerStream(peerId)
        }
    }

    fun setFullscreenStream(fullscreen: Boolean) {
        _uiState.update { it.copy(isFullscreenStream = fullscreen) }
    }

    fun setPipMode(inPip: Boolean) {
        _uiState.update { it.copy(isInPipMode = inPip) }
    }

    fun setChatOpen(open: Boolean) {
        _uiState.update {
            it.copy(
                isChatOpen = open,
                unreadChatCount = if (open) 0 else it.unreadChatCount,
            )
        }
    }

    fun sendChatText(text: String) {
        val trimmed = text.trim()
        if (trimmed.isEmpty()) return
        scope.launch {
            controller.sendChatText(trimmed)
        }
    }

    fun sendInlineImage(filename: String, bytes: ByteArray) {
        scope.launch {
            controller.sendInlineImage(filename, bytes)
        }
    }

    fun offerFile(filePath: String) {
        scope.launch {
            controller.offerFile(filePath)
        }
    }

    fun downloadBlob(blobIdHex: String) {
        runAction {
            controller.downloadBlob(blobIdHex)
        }
    }

    fun dismissBanner() {
        _uiState.update { it.copy(bannerMessage = null) }
    }

    fun setLanDiscoveryMulticastActive(active: Boolean) {
        VoiceCallForegroundService.setLobbyMulticastLock(appContext, active)
    }

    private fun clearRoomDrafts() {
        _uiState.update {
            it.copy(
                chatMessages = emptyList(),
                unreadChatCount = 0,
                watchedPeerId = null,
                activeShortCode = null,
                shortCodeSecondsRemaining = 0,
                invitedFriendsSession = emptySet(),
                lanAuthError = null,
                bannerMessage = null,
            )
        }
    }

    private inline fun runAction(crossinline block: () -> Unit) {
        scope.launch {
            try {
                block()
            } catch (e: MobileCoreException) {
                _uiState.update { it.copy(bannerMessage = e.message) }
            } catch (e: Throwable) {
                _uiState.update { it.copy(bannerMessage = e.message ?: "Unexpected error") }
            }
        }
    }

    private fun startTelemetryLoop() {
        telemetryJob?.cancel()
        telemetryJob = scope.launch {
            while (isActive && _uiState.value.inRoom) {
                _telemetry.value = controller.pollFastAudioTelemetry()
                delay(60L)
            }
        }
    }

    private fun stopTelemetryLoop() {
        telemetryJob?.cancel()
        telemetryJob = null
    }

    private fun onLeftRoomInternal() {
        stopTelemetryLoop()
        shortCodeTimerJob?.cancel()
        audioRouteController.deactivate()
        VoiceCallForegroundService.stop(appContext)
        _uiState.update {
            it.copy(
                inRoom = false,
                localPeerId = 0u,
                activeInviteUri = "",
                isLanLocked = false,
                peers = emptyList(),
                watchedPeerId = null,
                chatMessages = emptyList(),
                unreadChatCount = 0,
                isChatOpen = false,
                activeShortCode = null,
                shortCodeSecondsRemaining = 0,
                invitedFriendsSession = emptySet(),
                isFullscreenStream = false,
                isInPipMode = false,
            )
        }
    }

    private fun registerConnectivityWatcher() {
        if (networkCallbackRegistered) return
        val cm = appContext.getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager ?: return
        networkCallbackRegistered = true
        val request = NetworkRequest.Builder()
            .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .build()
        try {
            cm.registerNetworkCallback(
                request,
                object : ConnectivityManager.NetworkCallback() {
                    override fun onAvailable(network: Network) {
                        pushCurrentLanIpv4s()
                    }

                    override fun onLinkPropertiesChanged(network: Network, linkProperties: LinkProperties) {
                        pushCurrentLanIpv4s()
                    }

                    override fun onLost(network: Network) {
                        pushCurrentLanIpv4s()
                    }
                },
            )
        } catch (_: SecurityException) {
        }
    }

    private fun pushCurrentLanIpv4s() {
        val cm = appContext.getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager ?: return
        val addrs = mutableListOf<String>()
        try {
            @Suppress("DEPRECATION")
            for (network in cm.allNetworks) {
                val caps = cm.getNetworkCapabilities(network) ?: continue
                if (!caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) &&
                    !caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)
                ) {
                    continue
                }
                val lp = cm.getLinkProperties(network) ?: continue
                for (la in lp.linkAddresses) {
                    val addr = la.address
                    if (addr is Inet4Address && !addr.isLoopbackAddress && !addr.isAnyLocalAddress) {
                        addr.hostAddress?.let { addrs.add(it) }
                    }
                }
            }
        } catch (_: SecurityException) {
        }
        controller.setLanIpv4Override(addrs)
    }

    // MobileEventListener callbacks (invoked from Rust Tokio worker threads)

    override fun onJoinedRoom(localPeerId: UShort, inviteUri: String, isLanLocked: Boolean) {
        val alreadyInRoom = _uiState.value.inRoom
        _uiState.update {
            it.copy(
                inRoom = true,
                localPeerId = localPeerId,
                activeInviteUri = inviteUri,
                isLanLocked = isLanLocked,
                lanAuthError = null,
            )
        }
        if (!alreadyInRoom) {
            audioRouteController.activate()
            startTelemetryLoop()
            VoiceCallForegroundService.start(appContext)
        } else {
            VoiceCallForegroundService.updateNotificationIfRunning(appContext)
        }
    }

    override fun onLanAuthFailed(message: String) {
        _uiState.update { it.copy(lanAuthError = message, bannerMessage = message) }
    }

    override fun onLeftRoom() {
        onLeftRoomInternal()
    }

    override fun onPeersUpdated(peers: List<MobilePeerSnapshot>) {
        _uiState.update { st ->
            val updatedAvatars = st.peerAvatarsRgba.toMutableMap()
            for (p in peers) {
                if (!updatedAvatars.containsKey(p.clientUuidHex)) {
                    updatedAvatars[p.clientUuidHex] = controller.getIdenticonRgba(p.clientUuidHex)
                }
            }
            val currentWatched = st.watchedPeerId
            val watchedStillPresent = currentWatched != null && peers.any {
                it.peerId == currentWatched && it.streamInfo != null
            }
            st.copy(
                peers = peers,
                peerAvatarsRgba = updatedAvatars,
                watchedPeerId = if (watchedStillPresent) currentWatched else if (currentWatched != null && peers.none { it.peerId == currentWatched }) null else currentWatched,
            )
        }
        VoiceCallForegroundService.updateNotificationIfRunning(appContext)
    }

    override fun onChatItemReceived(item: MobileChatItem) {
        _uiState.update { st ->
            if (st.chatMessages.any { it.senderUuidHex == item.senderUuidHex && it.msgId == item.msgId }) {
                st
            } else {
                val nextList = (st.chatMessages + item).takeLast(60)
                val unreadDelta = if (!item.isSelf && !st.isChatOpen) 1 else 0
                st.copy(
                    chatMessages = nextList,
                    unreadChatCount = st.unreadChatCount + unreadDelta,
                )
            }
        }
    }

    override fun onBlobStateChanged(
        blobIdHex: String,
        state: MobileBlobTransferState,
        inlineBytes: ByteArray?,
    ) {
        _uiState.update { st ->
            val updated = st.chatMessages.map { msg ->
                when (val c = msg.content) {
                    is MobileChatContent.InlineImage -> if (c.blobIdHex == blobIdHex) {
                        msg.copy(
                            content = c.copy(
                                state = state,
                                bytes = inlineBytes ?: c.bytes,
                            ),
                        )
                    } else msg

                    is MobileChatContent.File -> if (c.blobIdHex == blobIdHex) {
                        msg.copy(content = c.copy(state = state))
                    } else msg

                    is MobileChatContent.Text -> msg
                }
            }
            st.copy(chatMessages = updated)
        }
    }

    override fun onPeerAvatarLoaded(
        clientUuidHex: String,
        avatarHashHex: String,
        rgba128: ByteArray,
    ) {
        _uiState.update { st ->
            st.copy(peerAvatarsRgba = st.peerAvatarsRgba + (clientUuidHex to rgba128))
        }
    }

    override fun onRemoteStreamUpdated(peerId: UShort, stream: MobileRemoteStreamInfo?) {
        _uiState.update { st ->
            val clearWatched = st.watchedPeerId == peerId && stream == null
            st.copy(
                peers = st.peers.map { p ->
                    if (p.peerId == peerId) p.copy(streamInfo = stream) else p
                },
                watchedPeerId = if (clearWatched) null else st.watchedPeerId,
                isFullscreenStream = if (clearWatched) false else st.isFullscreenStream,
            )
        }
    }

    override fun onLanRoomsUpdated(rooms: List<MobileLanRoomItem>) {
        _uiState.update { it.copy(lanRooms = rooms) }
    }

    override fun onFriendsStatusUpdated(friends: List<MobileFriendItem>) {
        _uiState.update { it.copy(friends = friends) }
    }

    override fun onIncomingFriendCall(call: MobileIncomingCall) {
        _uiState.update { it.copy(incomingCall = call) }
    }

    override fun onShortCodeGenerated(code: String, expiresInSecs: UShort) {
        shortCodeTimerJob?.cancel()
        val secs = expiresInSecs.toInt()
        _uiState.update {
            it.copy(
                activeShortCode = code,
                shortCodeSecondsRemaining = secs,
            )
        }
        shortCodeTimerJob = scope.launch {
            var remaining = secs
            while (isActive && remaining > 0) {
                delay(1000L)
                remaining -= 1
                _uiState.update { st ->
                    if (st.activeShortCode == code) {
                        st.copy(shortCodeSecondsRemaining = remaining)
                    } else st
                }
            }
        }
    }

    override fun onShortCodeCleared() {
        shortCodeTimerJob?.cancel()
        _uiState.update { it.copy(activeShortCode = null, shortCodeSecondsRemaining = 0) }
    }

    override fun onFriendInviteSent(pubKeyHex: String) {
        _uiState.update { st ->
            st.copy(invitedFriendsSession = st.invitedFriendsSession + pubKeyHex)
        }
    }

    override fun onError(message: String) {
        _uiState.update { it.copy(bannerMessage = message) }
    }

    companion object {
        @Volatile
        private var instance: SquadSessionRepository? = null

        fun initialize(context: Context): SquadSessionRepository {
            val existing = instance
            if (existing != null) {
                existing.startIfNeeded()
                return existing
            }
            return synchronized(this) {
                instance ?: SquadSessionRepository(context.applicationContext).also {
                    instance = it
                    it.startIfNeeded()
                }
            }
        }

        fun getInstance(context: Context): SquadSessionRepository {
            return instance ?: initialize(context)
        }
    }
}
