package app.squadair.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.BluetoothAudio
import androidx.compose.material.icons.filled.CallEnd
import androidx.compose.material.icons.filled.Chat
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.Headset
import androidx.compose.material.icons.filled.HeadsetOff
import androidx.compose.material.icons.filled.Key
import androidx.compose.material.icons.filled.Mic
import androidx.compose.material.icons.filled.MicOff
import androidx.compose.material.icons.filled.PersonAdd
import androidx.compose.material.icons.filled.PhoneInTalk
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material.icons.filled.VolumeUp
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.squadair.mobile.core.SquadSessionRepository
import app.squadair.mobile.core.SquadUiState
import app.squadair.mobile.service.AudioRoute
import app.squadair.mobile.ui.theme.RgbaAvatarImage
import app.squadair.mobile.ui.theme.SquadAirBackground
import app.squadair.mobile.ui.theme.SquadAirDangerRed
import app.squadair.mobile.ui.theme.SquadAirMeshCyan
import app.squadair.mobile.ui.theme.SquadAirOnPrimary
import app.squadair.mobile.ui.theme.SquadAirPrimary
import app.squadair.mobile.ui.theme.SquadAirRelayYellow
import app.squadair.mobile.ui.theme.SquadAirSpeakingGreen
import app.squadair.mobile.ui.theme.SquadAirSurfaceCard
import app.squadair.mobile.ui.theme.SquadAirSurfaceElevated
import app.squadair.mobile.ui.theme.SquadAirTextMuted
import app.squadair.mobile.ui.theme.SquadAirTextPrimary
import uniffi.squad_core.MobileAudioTelemetry
import uniffi.squad_core.MobilePeerSnapshot

@Composable
fun RoomScreen(
    state: SquadUiState,
    telemetry: MobileAudioTelemetry,
    repository: SquadSessionRepository,
) {
    val watchedPeer = state.peers.firstOrNull { it.peerId == state.watchedPeerId }
    if (state.isFullscreenStream && watchedPeer != null) {
        StreamStageView(
            peer = watchedPeer,
            streamVolume = state.profile.streamVolume,
            isFullscreen = true,
            isPipMode = false,
            onToggleFullscreen = { repository.setFullscreenStream(false) },
            onCloseStream = { repository.watchPeerStream(null) },
            onStreamVolumeChange = { repository.setStreamVolume(it) },
        )
        return
    }

    var shortCodeDialogOpen by remember { mutableStateOf(false) }
    var inviteFriendDialogOpen by remember { mutableStateOf(false) }
    var expandedPeerUuid by remember { mutableStateOf<String?>(null) }

    Scaffold(
        containerColor = SquadAirBackground,
        topBar = {
            RoomTopBar(
                state = state,
                repository = repository,
                onShowShortCode = {
                    repository.generateShortCode()
                    shortCodeDialogOpen = true
                },
            )
        },
        bottomBar = {
            RoomBottomBar(
                state = state,
                telemetry = telemetry,
                repository = repository,
                onOpenChat = { repository.setChatOpen(true) },
                onInviteFriend = { inviteFriendDialogOpen = true },
            )
        },
    ) { innerPadding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(innerPadding),
        ) {
            if (state.watchedPeerId != null) {
                StreamStageView(
                    peer = watchedPeer,
                    streamVolume = state.profile.streamVolume,
                    isFullscreen = false,
                    isPipMode = false,
                    onToggleFullscreen = { repository.setFullscreenStream(true) },
                    onCloseStream = { repository.watchPeerStream(null) },
                    onStreamVolumeChange = { repository.setStreamVolume(it) },
                )
            }

            state.bannerMessage?.let { msg ->
                Card(
                    colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceElevated),
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 16.dp, vertical = 8.dp),
                ) {
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(horizontal = 12.dp, vertical = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(
                            text = msg,
                            color = SquadAirDangerRed,
                            fontSize = 12.sp,
                            modifier = Modifier.weight(1f),
                        )
                        TextButton(onClick = { repository.dismissBanner() }) {
                            Text("OK", color = SquadAirPrimary)
                        }
                    }
                }
            }

            LazyColumn(
                modifier = Modifier.fillMaxSize(),
                contentPadding = PaddingValues(16.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                item {
                    SelfVoiceCard(state = state, telemetry = telemetry)
                }

                items(state.peers, key = { it.clientUuidHex }) { peer ->
                    PeerCard(
                        peer = peer,
                        avatarRgba = state.peerAvatarsRgba[peer.clientUuidHex],
                        isExpanded = expandedPeerUuid == peer.clientUuidHex,
                        isWatching = state.watchedPeerId == peer.peerId,
                        onToggleExpand = {
                            expandedPeerUuid = if (expandedPeerUuid == peer.clientUuidHex) null else peer.clientUuidHex
                        },
                        onVolumeChange = { gain -> repository.setPeerVolume(peer.clientUuidHex, gain) },
                        onToggleWatch = {
                            if (state.watchedPeerId == peer.peerId) {
                                repository.watchPeerStream(null)
                            } else {
                                repository.watchPeerStream(peer.peerId)
                            }
                        },
                    )
                }

                if (state.peers.isEmpty()) {
                    item {
                        Card(
                            colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceCard),
                            shape = RoundedCornerShape(14.dp),
                        ) {
                            Text(
                                text = "Waiting for friends to join. Share the 6-digit code or invite link above.",
                                color = SquadAirTextMuted,
                                fontSize = 13.sp,
                                modifier = Modifier.padding(16.dp),
                            )
                        }
                    }
                }
            }
        }
    }

    if (state.isChatOpen) {
        ChatSheet(
            state = state,
            repository = repository,
            onDismiss = { repository.setChatOpen(false) },
        )
    }

    if (shortCodeDialogOpen && state.activeShortCode != null) {
        val clipboard = LocalClipboardManager.current
        val formatted = state.activeShortCode.chunked(3).joinToString(" ")
        AlertDialog(
            onDismissRequest = { shortCodeDialogOpen = false },
            containerColor = SquadAirSurfaceCard,
            title = { Text("6-Digit Room Code (SPAKE2)", color = SquadAirTextPrimary) },
            text = {
                Column(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Text(
                        text = formatted,
                        color = SquadAirPrimary,
                        fontFamily = FontFamily.Monospace,
                        fontSize = 34.sp,
                        fontWeight = FontWeight.Bold,
                    )
                    Text(
                        text = "Expires in ${state.shortCodeSecondsRemaining}s",
                        color = SquadAirTextMuted,
                        fontSize = 13.sp,
                    )
                }
            },
            confirmButton = {
                Button(onClick = { clipboard.setText(AnnotatedString(state.activeShortCode)) }) {
                    Text("Copy Code")
                }
            },
            dismissButton = {
                TextButton(onClick = { shortCodeDialogOpen = false }) {
                    Text("Close")
                }
            },
        )
    }

    if (inviteFriendDialogOpen) {
        InviteFriendDialog(
            state = state,
            repository = repository,
            onDismiss = { inviteFriendDialogOpen = false },
        )
    }
}

@Composable
private fun RoomTopBar(
    state: SquadUiState,
    repository: SquadSessionRepository,
    onShowShortCode: () -> Unit,
) {
    val clipboard = LocalClipboardManager.current
    var routeMenuOpen by remember { mutableStateOf(false) }

    Surface(color = SquadAirSurfaceCard, tonalElevation = 2.dp) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .statusBarsPadding()
                .padding(horizontal = 12.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                modifier = Modifier
                    .size(10.dp)
                    .clip(CircleShape)
                    .background(SquadAirSpeakingGreen),
            )
            Spacer(modifier = Modifier.width(8.dp))
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text = "Voice Connected",
                    color = SquadAirTextPrimary,
                    fontSize = 15.sp,
                    fontWeight = FontWeight.SemiBold,
                )
                Text(
                    text = "${state.peers.size + 1} in room • E2EE",
                    color = SquadAirTextMuted,
                    fontSize = 11.sp,
                )
            }

            IconButton(onClick = onShowShortCode) {
                Icon(Icons.Default.Key, contentDescription = "6-Digit Code", tint = SquadAirPrimary)
            }
            IconButton(onClick = { clipboard.setText(AnnotatedString(state.activeInviteUri)) }) {
                Icon(Icons.Default.ContentCopy, contentDescription = "Copy Invite Link", tint = SquadAirPrimary)
            }
            Box {
                val routeIcon = when (state.audioRoute) {
                    AudioRoute.BLUETOOTH -> Icons.Default.BluetoothAudio
                    AudioRoute.EARPIECE -> Icons.Default.PhoneInTalk
                    AudioRoute.WIRED_HEADSET -> Icons.Default.Headset
                    AudioRoute.SPEAKER -> Icons.Default.VolumeUp
                }
                IconButton(onClick = { routeMenuOpen = true }) {
                    Icon(routeIcon, contentDescription = "Audio Route", tint = SquadAirPrimary)
                }
                DropdownMenu(
                    expanded = routeMenuOpen,
                    onDismissRequest = { routeMenuOpen = false },
                    modifier = Modifier.background(SquadAirSurfaceElevated),
                ) {
                    state.availableAudioRoutes.forEach { route ->
                        DropdownMenuItem(
                            text = {
                                Text(
                                    text = if (route == state.audioRoute) "✓ ${route.label}" else route.label,
                                    color = if (route == state.audioRoute) SquadAirPrimary else SquadAirTextPrimary,
                                )
                            },
                            onClick = {
                                repository.setAudioRoute(route)
                                routeMenuOpen = false
                            },
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun SelfVoiceCard(
    state: SquadUiState,
    telemetry: MobileAudioTelemetry,
) {
    val avatar = state.peerAvatarsRgba[state.profile.clientUuidHex]
    val speaking = telemetry.isLocalSpeaking && !state.profile.micMuted && !state.profile.deafened

    Card(
        colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceCard),
        shape = RoundedCornerShape(14.dp),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            RgbaAvatarImage(rgbaBytes = avatar, sizeDp = 42.dp, isSpeaking = speaking)
            Spacer(modifier = Modifier.width(12.dp))
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text = "${state.profile.nickname} (You)",
                    color = SquadAirTextPrimary,
                    fontSize = 15.sp,
                    fontWeight = FontWeight.SemiBold,
                )
                Text(
                    text = when {
                        state.profile.deafened -> "Deafened"
                        state.profile.micMuted -> "Microphone muted"
                        speaking -> "Speaking • Jitter ${telemetry.jitterTargetMs} ms"
                        else -> "Mic ready • Route: ${state.audioRoute.label}"
                    },
                    color = if (speaking) SquadAirSpeakingGreen else SquadAirTextMuted,
                    fontSize = 12.sp,
                )
            }
        }
    }
}

@Composable
private fun PeerCard(
    peer: MobilePeerSnapshot,
    avatarRgba: ByteArray?,
    isExpanded: Boolean,
    isWatching: Boolean,
    onToggleExpand: () -> Unit,
    onVolumeChange: (Float) -> Unit,
    onToggleWatch: () -> Unit,
) {
    val (routeBadgeText, routeBadgeColor) = when (peer.route) {
        "LAN" -> "LAN • ${peer.rttMs} ms" to SquadAirSpeakingGreen
        "P2P" -> "P2P • ${peer.rttMs} ms" to SquadAirSpeakingGreen
        "MESH" -> "MESH • ${peer.rttMs} ms" to SquadAirMeshCyan
        "RELAY" -> "RELAY • ${peer.rttMs} ms" to SquadAirRelayYellow
        "NAT_FAILED" -> "NAT FAILED" to SquadAirDangerRed
        else -> "CONNECTING" to SquadAirTextMuted
    }

    Card(
        colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceCard),
        shape = RoundedCornerShape(14.dp),
        modifier = Modifier
            .fillMaxWidth()
            .clickable { onToggleExpand() },
    ) {
        Column(modifier = Modifier.padding(12.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                RgbaAvatarImage(
                    rgbaBytes = avatarRgba,
                    sizeDp = 42.dp,
                    isSpeaking = peer.isSpeaking,
                )
                Spacer(modifier = Modifier.width(12.dp))
                Column(modifier = Modifier.weight(1f)) {
                    Text(
                        text = peer.nickname,
                        color = SquadAirTextPrimary,
                        fontSize = 15.sp,
                        fontWeight = FontWeight.Medium,
                    )
                    Text(
                        text = routeBadgeText,
                        color = routeBadgeColor,
                        fontSize = 12.sp,
                        fontWeight = FontWeight.Medium,
                    )
                }

                peer.streamInfo?.let { info ->
                    Column(horizontalAlignment = Alignment.End) {
                        Surface(
                            color = SquadAirDangerRed.copy(alpha = 0.2f),
                            shape = RoundedCornerShape(6.dp),
                        ) {
                            Text(
                                text = "LIVE • ${info.height}p${info.fps}",
                                color = SquadAirDangerRed,
                                fontSize = 10.sp,
                                fontWeight = FontWeight.Bold,
                                modifier = Modifier.padding(horizontal = 6.dp, vertical = 2.dp),
                            )
                        }
                        Spacer(modifier = Modifier.height(4.dp))
                        FilledTonalButton(
                            onClick = onToggleWatch,
                            contentPadding = PaddingValues(horizontal = 10.dp, vertical = 4.dp),
                        ) {
                            Icon(
                                imageVector = if (isWatching) Icons.Default.Stop else Icons.Default.PlayArrow,
                                contentDescription = null,
                                modifier = Modifier.size(16.dp),
                            )
                            Spacer(modifier = Modifier.width(4.dp))
                            Text(if (isWatching) "Stop" else "Watch Stream", fontSize = 12.sp)
                        }
                    }
                }
            }

            if (isExpanded) {
                Spacer(modifier = Modifier.height(8.dp))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Icon(
                        imageVector = Icons.Default.VolumeUp,
                        contentDescription = null,
                        tint = SquadAirPrimary,
                        modifier = Modifier.size(18.dp),
                    )
                    Spacer(modifier = Modifier.width(8.dp))
                    Slider(
                        value = peer.volumeGain.coerceIn(0f, 2f),
                        onValueChange = onVolumeChange,
                        valueRange = 0f..2f,
                        colors = SliderDefaults.colors(
                            thumbColor = SquadAirPrimary,
                            activeTrackColor = SquadAirPrimary,
                        ),
                        modifier = Modifier.weight(1f),
                    )
                    Text(
                        text = "${(peer.volumeGain * 100).toInt()}%",
                        color = SquadAirTextPrimary,
                        fontSize = 12.sp,
                    )
                }
            }
        }
    }
}

@Composable
private fun RoomBottomBar(
    state: SquadUiState,
    telemetry: MobileAudioTelemetry,
    repository: SquadSessionRepository,
    onOpenChat: () -> Unit,
    onInviteFriend: () -> Unit,
) {
    val micLevelNorm = ((telemetry.localMicLevelDb + 60f) / 60f).coerceIn(0f, 1f)

    Surface(color = SquadAirSurfaceCard, tonalElevation = 4.dp) {
        Column(modifier = Modifier.navigationBarsPadding()) {
            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .height(3.dp)
                    .background(SquadAirSurfaceElevated),
            ) {
                Box(
                    modifier = Modifier
                        .fillMaxWidth(if (state.profile.micMuted) 0f else micLevelNorm)
                        .fillMaxHeight()
                        .background(if (telemetry.isLocalSpeaking) SquadAirSpeakingGreen else SquadAirPrimary),
                )
            }
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 12.dp, vertical = 10.dp),
                horizontalArrangement = Arrangement.SpaceEvenly,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Button(
                    onClick = { repository.toggleMicMute() },
                    colors = ButtonDefaults.buttonColors(
                        containerColor = if (state.profile.micMuted) SquadAirDangerRed else SquadAirSurfaceElevated,
                        contentColor = if (state.profile.micMuted) Color.Black else SquadAirTextPrimary,
                    ),
                    shape = RoundedCornerShape(14.dp),
                ) {
                    Icon(
                        imageVector = if (state.profile.micMuted) Icons.Default.MicOff else Icons.Default.Mic,
                        contentDescription = "Toggle mic",
                    )
                    Spacer(modifier = Modifier.width(6.dp))
                    Text(if (state.profile.micMuted) "Unmute" else "Mute")
                }

                IconButton(onClick = { repository.toggleDeafen() }) {
                    Icon(
                        imageVector = if (state.profile.deafened) Icons.Default.HeadsetOff else Icons.Default.Headset,
                        contentDescription = "Deafen",
                        tint = if (state.profile.deafened) SquadAirDangerRed else SquadAirTextPrimary,
                    )
                }

                IconButton(onClick = onOpenChat) {
                    BadgedBox(
                        badge = {
                            if (state.unreadChatCount > 0) {
                                Badge(containerColor = SquadAirPrimary, contentColor = SquadAirOnPrimary) {
                                    Text("${state.unreadChatCount}")
                                }
                            }
                        },
                    ) {
                        Icon(
                            imageVector = Icons.Default.Chat,
                            contentDescription = "Chat",
                            tint = SquadAirTextPrimary,
                        )
                    }
                }

                IconButton(onClick = onInviteFriend) {
                    Icon(
                        imageVector = Icons.Default.PersonAdd,
                        contentDescription = "Invite Friend",
                        tint = SquadAirPrimary,
                    )
                }

                Button(
                    onClick = { repository.leaveRoom() },
                    colors = ButtonDefaults.buttonColors(
                        containerColor = SquadAirDangerRed,
                        contentColor = Color.Black,
                    ),
                    shape = RoundedCornerShape(14.dp),
                ) {
                    Icon(Icons.Default.CallEnd, contentDescription = "Leave room")
                }
            }
        }
    }
}

@Composable
private fun InviteFriendDialog(
    state: SquadUiState,
    repository: SquadSessionRepository,
    onDismiss: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = SquadAirSurfaceCard,
        title = { Text("Invite Friend to Room", color = SquadAirTextPrimary) },
        text = {
            if (state.friends.isEmpty()) {
                Text(
                    text = "Add friends in the Lobby Friends tab to send encrypted X25519 invites.",
                    color = SquadAirTextMuted,
                    fontSize = 13.sp,
                )
            } else {
                LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    items(state.friends, key = { it.pubKeyHex }) { friend ->
                        val invited = state.invitedFriendsSession.contains(friend.pubKeyHex)
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Column(modifier = Modifier.weight(1f)) {
                                Text(friend.nickname, color = SquadAirTextPrimary, fontSize = 14.sp)
                                Text(
                                    text = if (friend.online) "Online" else "Offline",
                                    color = if (friend.online) SquadAirSpeakingGreen else SquadAirTextMuted,
                                    fontSize = 11.sp,
                                )
                            }
                            FilledTonalButton(
                                onClick = { repository.callOrInviteFriend(friend.pubKeyHex) },
                                enabled = friend.online && !invited,
                            ) {
                                Text(if (invited) "Sent" else "Invite")
                            }
                        }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) {
                Text("Close")
            }
        },
    )
}
