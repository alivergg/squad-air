package app.squadair.mobile.ui

import android.content.Intent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Call
import androidx.compose.material.icons.filled.ContentPaste
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Group
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.Lock
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Share
import androidx.compose.material.icons.filled.Wifi
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.squadair.mobile.core.AppUpdater
import app.squadair.mobile.core.SquadSessionRepository
import app.squadair.mobile.core.SquadUiState
import app.squadair.mobile.core.UpdateState
import app.squadair.mobile.ui.theme.RgbaAvatarImage
import app.squadair.mobile.ui.theme.SquadAirBackground
import app.squadair.mobile.ui.theme.SquadAirDangerRed
import app.squadair.mobile.ui.theme.SquadAirMeshCyan
import app.squadair.mobile.ui.theme.SquadAirOnPrimary
import app.squadair.mobile.ui.theme.SquadAirPrimary
import app.squadair.mobile.ui.theme.SquadAirSpeakingGreen
import app.squadair.mobile.ui.theme.SquadAirSurfaceCard
import app.squadair.mobile.ui.theme.SquadAirSurfaceElevated
import app.squadair.mobile.ui.theme.SquadAirTextMuted
import app.squadair.mobile.ui.theme.SquadAirTextPrimary
import uniffi.squad_core.MobileLanRoomItem

@Composable
fun LobbyScreen(
    state: SquadUiState,
    repository: SquadSessionRepository,
) {
    var selectedTab by remember { mutableIntStateOf(0) }
    var settingsOpen by remember { mutableStateOf(false) }
    var passwordPromptRoom by remember { mutableStateOf<MobileLanRoomItem?>(null) }

    Scaffold(
        containerColor = SquadAirBackground,
        topBar = {
            LobbyTopBar(
                state = state,
                selfAvatarRgba = state.peerAvatarsRgba[state.profile.clientUuidHex],
                onOpenSettings = { settingsOpen = true },
            )
        },
        bottomBar = {
            NavigationBar(containerColor = SquadAirSurfaceCard) {
                NavigationBarItem(
                    selected = selectedTab == 0,
                    onClick = { selectedTab = 0 },
                    icon = { Icon(Icons.Default.Home, contentDescription = "Rooms") },
                    label = { Text("Rooms") },
                )
                NavigationBarItem(
                    selected = selectedTab == 1,
                    onClick = { selectedTab = 1 },
                    icon = { Icon(Icons.Default.Group, contentDescription = "Friends") },
                    label = { Text("Friends") },
                )
                NavigationBarItem(
                    selected = selectedTab == 2,
                    onClick = { selectedTab = 2 },
                    icon = { Icon(Icons.Default.Wifi, contentDescription = "LAN") },
                    label = { Text("LAN") },
                )
            }
        },
    ) { innerPadding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(innerPadding)
                .padding(horizontal = 16.dp),
        ) {
            state.bannerMessage?.let { msg ->
                Card(
                    colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceElevated),
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(vertical = 8.dp),
                ) {
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(horizontal = 12.dp, vertical = 10.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(
                            text = msg,
                            color = SquadAirDangerRed,
                            fontSize = 13.sp,
                            modifier = Modifier.weight(1f),
                        )
                        TextButton(onClick = { repository.dismissBanner() }) {
                            Text("Dismiss", color = SquadAirPrimary)
                        }
                    }
                }
            }

            if (!state.updateBannerDismissed) {
                LobbyUpdateBanner(
                    updateState = state.updateState,
                    repository = repository,
                )
            }

            when (selectedTab) {
                0 -> QuickJoinTab(state = state, repository = repository)
                1 -> FriendsTab(state = state, repository = repository)
                2 -> LanRoomsTab(
                    state = state,
                    repository = repository,
                    onRoomClicked = { room ->
                        if (room.requiresPassword) {
                            repository.clearLanAuthError()
                            passwordPromptRoom = room
                        } else {
                            repository.joinLanRoom(room.roomIdHex, null)
                        }
                    },
                )
            }
        }
    }

    if (settingsOpen) {
        SettingsSheet(
            profile = state.profile,
            selfAvatarRgba = state.peerAvatarsRgba[state.profile.clientUuidHex],
            repository = repository,
            onDismiss = { settingsOpen = false },
        )
    }

    passwordPromptRoom?.let { room ->
        LanPasswordDialog(
            room = room,
            error = state.lanAuthError,
            onConfirm = { pw ->
                repository.joinLanRoom(room.roomIdHex, pw)
            },
            onDismiss = {
                repository.clearLanAuthError()
                passwordPromptRoom = null
            },
        )
    }
}

@Composable
private fun LobbyTopBar(
    state: SquadUiState,
    selfAvatarRgba: ByteArray?,
    onOpenSettings: () -> Unit,
) {
    Surface(color = SquadAirSurfaceCard, tonalElevation = 2.dp) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .statusBarsPadding()
                .padding(horizontal = 16.dp, vertical = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            RgbaAvatarImage(
                rgbaBytes = selfAvatarRgba,
                sizeDp = 42.dp,
                modifier = Modifier.clickable(onClick = onOpenSettings),
            )
            Spacer(modifier = Modifier.width(12.dp))
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text = state.profile.nickname,
                    color = SquadAirTextPrimary,
                    fontSize = 17.sp,
                    fontWeight = FontWeight.SemiBold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box(
                        modifier = Modifier
                            .size(8.dp)
                            .clip(CircleShape)
                            .background(SquadAirSpeakingGreen),
                    )
                    Spacer(modifier = Modifier.width(6.dp))
                    Text(
                        text = "${state.profile.rendezvousServer}:3478",
                        color = SquadAirTextMuted,
                        fontSize = 12.sp,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
            IconButton(onClick = onOpenSettings) {
                Icon(
                    imageVector = Icons.Default.Settings,
                    contentDescription = "Settings",
                    tint = SquadAirPrimary,
                )
            }
        }
    }
}

@Composable
private fun QuickJoinTab(
    state: SquadUiState,
    repository: SquadSessionRepository,
) {
    val clipboard = LocalClipboardManager.current
    var shortCodeInput by remember { mutableStateOf("") }
    var inviteInput by remember { mutableStateOf("") }
    var saveDialogOpen by remember { mutableStateOf(false) }

    LazyColumn(
        modifier = Modifier.fillMaxSize(),
        contentPadding = PaddingValues(vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        item {
            Button(
                onClick = { repository.createAndJoinRoom() },
                colors = ButtonDefaults.buttonColors(
                    containerColor = SquadAirPrimary,
                    contentColor = SquadAirOnPrimary,
                ),
                shape = RoundedCornerShape(14.dp),
                modifier = Modifier
                    .fillMaxWidth()
                    .height(52.dp),
            ) {
                Icon(Icons.Default.Add, contentDescription = null)
                Spacer(modifier = Modifier.width(8.dp))
                Text("Create New Room", fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
            }
        }

        item {
            Card(
                colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceCard),
                shape = RoundedCornerShape(14.dp),
            ) {
                Column(modifier = Modifier.padding(14.dp)) {
                    Text(
                        text = "6-Digit Room Code (SPAKE2)",
                        color = SquadAirTextPrimary,
                        fontWeight = FontWeight.Medium,
                        fontSize = 14.sp,
                    )
                    Spacer(modifier = Modifier.height(8.dp))
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        OutlinedTextField(
                            value = shortCodeInput,
                            onValueChange = { raw ->
                                val filtered = raw.filter { it.isDigit() }.take(6)
                                shortCodeInput = filtered
                            },
                            placeholder = { Text("000 000") },
                            singleLine = true,
                            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                            modifier = Modifier.weight(1f),
                        )
                        Spacer(modifier = Modifier.width(8.dp))
                        FilledTonalButton(
                            onClick = { repository.joinRoomByShortCode(shortCodeInput) },
                            enabled = shortCodeInput.length == 6,
                        ) {
                            Text("Join by Code")
                        }
                    }
                }
            }
        }

        item {
            Card(
                colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceCard),
                shape = RoundedCornerShape(14.dp),
            ) {
                Column(modifier = Modifier.padding(14.dp)) {
                    Text(
                        text = "Invite Link (squad://...)",
                        color = SquadAirTextPrimary,
                        fontWeight = FontWeight.Medium,
                        fontSize = 14.sp,
                    )
                    Spacer(modifier = Modifier.height(8.dp))
                    OutlinedTextField(
                        value = inviteInput,
                        onValueChange = { inviteInput = it },
                        placeholder = { Text("squad://<room>#<secret>") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Spacer(modifier = Modifier.height(8.dp))
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        FilledTonalButton(
                            onClick = {
                                val clip = clipboard.getText()?.text?.trim().orEmpty()
                                if (clip.isNotEmpty()) {
                                    inviteInput = clip
                                    repository.joinRoomByInvite(clip)
                                }
                            },
                            modifier = Modifier.weight(1f),
                        ) {
                            Icon(Icons.Default.ContentPaste, contentDescription = null, modifier = Modifier.size(18.dp))
                            Spacer(modifier = Modifier.width(6.dp))
                            Text("Paste & Join")
                        }
                        Button(
                            onClick = { repository.joinRoomByInvite(inviteInput) },
                            enabled = inviteInput.isNotBlank(),
                            colors = ButtonDefaults.buttonColors(
                                containerColor = SquadAirPrimary,
                                contentColor = SquadAirOnPrimary,
                            ),
                            modifier = Modifier.weight(1f),
                        ) {
                            Text("Join")
                        }
                    }
                }
            }
        }

        item {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Text(
                    text = "Saved Channels",
                    color = SquadAirTextPrimary,
                    fontSize = 15.sp,
                    fontWeight = FontWeight.SemiBold,
                )
                TextButton(onClick = { saveDialogOpen = true }) {
                    Text("+ Save Link", color = SquadAirPrimary)
                }
            }
        }

        if (state.savedChannels.isEmpty()) {
            item {
                Text(
                    text = "No saved channels yet. Save an invite link for 1-tap access.",
                    color = SquadAirTextMuted,
                    fontSize = 13.sp,
                    modifier = Modifier.padding(vertical = 8.dp),
                )
            }
        } else {
            items(state.savedChannels, key = { it.inviteUri }) { ch ->
                Card(
                    colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceCard),
                    shape = RoundedCornerShape(12.dp),
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable { repository.joinRoomByInvite(ch.inviteUri) },
                ) {
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(horizontal = 14.dp, vertical = 12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Column(modifier = Modifier.weight(1f)) {
                            Text(
                                text = ch.name,
                                color = SquadAirTextPrimary,
                                fontSize = 15.sp,
                                fontWeight = FontWeight.Medium,
                            )
                            Text(
                                text = ch.inviteUri,
                                color = SquadAirTextMuted,
                                fontSize = 12.sp,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                        IconButton(onClick = { repository.removeSavedChannel(ch.inviteUri) }) {
                            Icon(
                                imageVector = Icons.Default.Delete,
                                contentDescription = "Delete channel",
                                tint = SquadAirTextMuted,
                            )
                        }
                    }
                }
            }
        }
    }

    if (saveDialogOpen) {
        var chName by remember { mutableStateOf("") }
        var chUri by remember { mutableStateOf("") }
        AlertDialog(
            onDismissRequest = { saveDialogOpen = false },
            containerColor = SquadAirSurfaceCard,
            title = { Text("Save Channel", color = SquadAirTextPrimary) },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(
                        value = chName,
                        onValueChange = { chName = it },
                        label = { Text("Channel Name") },
                        singleLine = true,
                    )
                    OutlinedTextField(
                        value = chUri,
                        onValueChange = { chUri = it },
                        label = { Text("Invite URI (squad://...)") },
                        singleLine = true,
                    )
                }
            },
            confirmButton = {
                Button(
                    onClick = {
                        repository.saveChannel(chName, chUri)
                        saveDialogOpen = false
                    },
                    enabled = chUri.isNotBlank(),
                ) {
                    Text("Save")
                }
            },
            dismissButton = {
                TextButton(onClick = { saveDialogOpen = false }) {
                    Text("Cancel")
                }
            },
        )
    }
}

@Composable
private fun FriendsTab(
    state: SquadUiState,
    repository: SquadSessionRepository,
) {
    val context = LocalContext.current
    var friendCodeInput by remember { mutableStateOf("") }

    LazyColumn(
        modifier = Modifier.fillMaxSize(),
        contentPadding = PaddingValues(vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        item {
            Card(
                colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceCard),
                shape = RoundedCornerShape(14.dp),
            ) {
                Column(modifier = Modifier.padding(14.dp)) {
                    Text(
                        text = "My Friend Code (X25519)",
                        color = SquadAirTextPrimary,
                        fontWeight = FontWeight.Medium,
                        fontSize = 14.sp,
                    )
                    Spacer(modifier = Modifier.height(6.dp))
                    Text(
                        text = state.profile.myFriendCode,
                        color = SquadAirPrimary,
                        fontFamily = FontFamily.Monospace,
                        fontSize = 12.sp,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Spacer(modifier = Modifier.height(10.dp))
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        AnimatedCopyButton(
                            textToCopy = state.profile.myFriendCode,
                            label = "Copy Code",
                            tonal = true,
                            modifier = Modifier.weight(1f),
                        )
                        FilledTonalButton(
                            onClick = {
                                val sendIntent = Intent(Intent.ACTION_SEND).apply {
                                    type = "text/plain"
                                    putExtra(Intent.EXTRA_TEXT, state.profile.myFriendCode)
                                }
                                context.startActivity(Intent.createChooser(sendIntent, "Share SquadAir Friend Code"))
                            },
                            modifier = Modifier.weight(1f),
                        ) {
                            Icon(Icons.Default.Share, contentDescription = null, modifier = Modifier.size(16.dp))
                            Spacer(modifier = Modifier.width(6.dp))
                            Text("Share")
                        }
                    }
                }
            }
        }

        item {
            Card(
                colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceCard),
                shape = RoundedCornerShape(14.dp),
            ) {
                Column(modifier = Modifier.padding(14.dp)) {
                    Text(
                        text = "Add Friend",
                        color = SquadAirTextPrimary,
                        fontWeight = FontWeight.Medium,
                        fontSize = 14.sp,
                    )
                    Spacer(modifier = Modifier.height(8.dp))
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        OutlinedTextField(
                            value = friendCodeInput,
                            onValueChange = { friendCodeInput = it },
                            placeholder = { Text("sqf_...") },
                            singleLine = true,
                            modifier = Modifier.weight(1f),
                        )
                        Spacer(modifier = Modifier.width(8.dp))
                        Button(
                            onClick = {
                                repository.addFriendByCode(friendCodeInput)
                                friendCodeInput = ""
                            },
                            enabled = friendCodeInput.isNotBlank(),
                            colors = ButtonDefaults.buttonColors(
                                containerColor = SquadAirPrimary,
                                contentColor = SquadAirOnPrimary,
                            ),
                        ) {
                            Text("Add")
                        }
                    }
                }
            }
        }

        if (state.friends.isEmpty()) {
            item {
                Text(
                    text = "No friends added yet. Exchange sqf_ codes to call friends directly.",
                    color = SquadAirTextMuted,
                    fontSize = 13.sp,
                )
            }
        } else {
            items(state.friends, key = { it.pubKeyHex }) { friend ->
                val avatar = remember(friend.pubKeyHex) { repository.getFriendIdenticon(friend.pubKeyHex) }
                val statusLabel = when {
                    friend.online && friend.inRoom -> "Online • In Room"
                    friend.online -> "Online"
                    else -> "Offline"
                }
                val statusColor = when {
                    friend.online && friend.inRoom -> SquadAirMeshCyan
                    friend.online -> SquadAirSpeakingGreen
                    else -> SquadAirTextMuted
                }
                Card(
                    colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceCard),
                    shape = RoundedCornerShape(12.dp),
                ) {
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        RgbaAvatarImage(rgbaBytes = avatar, sizeDp = 40.dp)
                        Spacer(modifier = Modifier.width(12.dp))
                        Column(modifier = Modifier.weight(1f)) {
                            Text(
                                text = friend.nickname,
                                color = SquadAirTextPrimary,
                                fontSize = 15.sp,
                                fontWeight = FontWeight.Medium,
                            )
                            Text(
                                text = statusLabel,
                                color = statusColor,
                                fontSize = 12.sp,
                            )
                        }
                        FilledTonalButton(
                            onClick = { repository.callOrInviteFriend(friend.pubKeyHex) },
                            enabled = friend.online,
                        ) {
                            Icon(Icons.Default.Call, contentDescription = null, modifier = Modifier.size(16.dp))
                            Spacer(modifier = Modifier.width(4.dp))
                            Text("Call")
                        }
                        IconButton(onClick = { repository.removeFriend(friend.pubKeyHex) }) {
                            Icon(
                                imageVector = Icons.Default.Delete,
                                contentDescription = "Remove friend",
                                tint = SquadAirTextMuted,
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun LanRoomsTab(
    state: SquadUiState,
    repository: SquadSessionRepository,
    onRoomClicked: (MobileLanRoomItem) -> Unit,
) {
    DisposableEffect(Unit) {
        repository.setLanDiscoveryMulticastActive(true)
        onDispose {
            repository.setLanDiscoveryMulticastActive(false)
        }
    }

    var lanRoomName by remember { mutableStateOf("") }
    var lanPassword by remember { mutableStateOf("") }

    LazyColumn(
        modifier = Modifier.fillMaxSize(),
        contentPadding = PaddingValues(vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        item {
            Card(
                colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceCard),
                shape = RoundedCornerShape(14.dp),
            ) {
                Column(
                    modifier = Modifier.padding(14.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Text(
                        text = "Host Local Wi-Fi Room",
                        color = SquadAirTextPrimary,
                        fontWeight = FontWeight.Medium,
                        fontSize = 14.sp,
                    )
                    OutlinedTextField(
                        value = lanRoomName,
                        onValueChange = { lanRoomName = it },
                        label = { Text("Room Name (optional)") },
                        placeholder = { Text("${state.profile.nickname}'s LAN") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )
                    OutlinedTextField(
                        value = lanPassword,
                        onValueChange = { lanPassword = it },
                        label = { Text("Password (optional)") },
                        singleLine = true,
                        visualTransformation = PasswordVisualTransformation(),
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Button(
                        onClick = {
                            repository.hostLanRoom(lanRoomName, lanPassword.ifBlank { null })
                            lanPassword = ""
                        },
                        colors = ButtonDefaults.buttonColors(
                            containerColor = SquadAirPrimary,
                            contentColor = SquadAirOnPrimary,
                        ),
                        shape = RoundedCornerShape(12.dp),
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Icon(Icons.Default.Add, contentDescription = null)
                        Spacer(modifier = Modifier.width(6.dp))
                        Text("Create LAN Room", fontWeight = FontWeight.SemiBold)
                    }
                }
            }
        }
        item {
            Text(
                text = "Local Wi-Fi Rooms (UDP 3479)",
                color = SquadAirTextPrimary,
                fontSize = 15.sp,
                fontWeight = FontWeight.SemiBold,
            )
        }
        if (state.lanRooms.isEmpty()) {
            item {
                Card(
                    colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceCard),
                    shape = RoundedCornerShape(14.dp),
                ) {
                    Text(
                        text = "Scanning local network for SquadAir LAN rooms...",
                        color = SquadAirTextMuted,
                        fontSize = 13.sp,
                        modifier = Modifier.padding(16.dp),
                    )
                }
            }
        } else {
            items(state.lanRooms, key = { it.roomIdHex }) { room ->
                Card(
                    colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceCard),
                    shape = RoundedCornerShape(14.dp),
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable { onRoomClicked(room) },
                ) {
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(14.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Column(modifier = Modifier.weight(1f)) {
                            Text(
                                text = room.roomName,
                                color = SquadAirTextPrimary,
                                fontSize = 15.sp,
                                fontWeight = FontWeight.SemiBold,
                            )
                            Text(
                                text = "Host: ${room.hostNickname} • ${room.peerCount}/${room.maxPeers} peers",
                                color = SquadAirTextMuted,
                                fontSize = 12.sp,
                            )
                        }
                        if (room.requiresPassword) {
                            Icon(
                                imageVector = Icons.Default.Lock,
                                contentDescription = "Password protected",
                                tint = SquadAirPrimary,
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun LobbyUpdateBanner(
    updateState: UpdateState,
    repository: SquadSessionRepository,
) {
    val context = LocalContext.current
    val installPermLauncher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.StartActivityForResult(),
    ) {
        repository.onInstallPermissionResult()
    }

    when (updateState) {
        is UpdateState.Available -> {
            Card(
                colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceElevated),
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(vertical = 6.dp),
            ) {
                Column(
                    modifier = Modifier.padding(12.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Text(
                        text = "SquadAir ${updateState.release.tagName} available",
                        color = SquadAirTextPrimary,
                        fontSize = 14.sp,
                        fontWeight = FontWeight.SemiBold,
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Button(
                            onClick = { repository.downloadAndInstallUpdate(updateState.release) },
                            colors = ButtonDefaults.buttonColors(
                                containerColor = SquadAirPrimary,
                                contentColor = SquadAirOnPrimary,
                            ),
                        ) {
                            Text("Download")
                        }
                        TextButton(onClick = { repository.dismissUpdateBanner() }) {
                            Text("Later", color = SquadAirTextMuted)
                        }
                    }
                }
            }
        }
        is UpdateState.Downloading -> {
            Card(
                colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceElevated),
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(vertical = 6.dp),
            ) {
                Column(
                    modifier = Modifier.padding(12.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    val pct = updateState.progress?.let { "${(it * 100).toInt()}%" } ?: "..."
                    Text(
                        text = "Downloading ${updateState.release.tagName} ($pct)",
                        color = SquadAirTextPrimary,
                        fontSize = 13.sp,
                    )
                    val progress = updateState.progress
                    if (progress != null) {
                        LinearProgressIndicator(
                            progress = { progress },
                            modifier = Modifier.fillMaxWidth(),
                        )
                    } else {
                        LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
                    }
                }
            }
        }
        is UpdateState.ReadyToInstall -> {
            Card(
                colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceElevated),
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(vertical = 6.dp),
            ) {
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.SpaceBetween,
                ) {
                    Text(
                        text = "${updateState.release.tagName} ready to install",
                        color = SquadAirSpeakingGreen,
                        fontSize = 13.sp,
                        modifier = Modifier.weight(1f),
                    )
                    Button(onClick = { repository.installDownloadedUpdate() }) {
                        Text("Install Update")
                    }
                }
            }
        }
        is UpdateState.NeedsInstallPermission -> {
            Card(
                colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceElevated),
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(vertical = 6.dp),
            ) {
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.SpaceBetween,
                ) {
                    Text(
                        text = "Allow app installs to finish updating to ${updateState.release.tagName}",
                        color = SquadAirTextPrimary,
                        fontSize = 12.sp,
                        modifier = Modifier.weight(1f),
                    )
                    Spacer(modifier = Modifier.width(8.dp))
                    Button(
                        onClick = {
                            installPermLauncher.launch(AppUpdater.createUnknownSourcesSettingsIntent(context))
                        },
                    ) {
                        Text("Allow & Install")
                    }
                }
            }
        }
        is UpdateState.Error -> {
            val failedRelease = updateState.release
            if (failedRelease != null) {
                Card(
                    colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceElevated),
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(vertical = 6.dp),
                ) {
                    Column(
                        modifier = Modifier.padding(12.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Text(
                            text = "Update ${failedRelease.tagName} failed: ${updateState.message}",
                            color = SquadAirDangerRed,
                            fontSize = 13.sp,
                        )
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Button(
                                onClick = { repository.downloadAndInstallUpdate(failedRelease) },
                                colors = ButtonDefaults.buttonColors(
                                    containerColor = SquadAirPrimary,
                                    contentColor = SquadAirOnPrimary,
                                ),
                            ) {
                                Text("Retry")
                            }
                            TextButton(onClick = { repository.dismissUpdateBanner() }) {
                                Text("Dismiss", color = SquadAirTextMuted)
                            }
                        }
                    }
                }
            }
        }
        else -> Unit
    }
}

@Composable
private fun LanPasswordDialog(
    room: MobileLanRoomItem,
    error: String?,
    onConfirm: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    var password by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = SquadAirSurfaceCard,
        title = { Text("Enter LAN Password", color = SquadAirTextPrimary) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    text = "Room \"${room.roomName}\" is protected by SPAKE2-LAN.",
                    color = SquadAirTextMuted,
                    fontSize = 13.sp,
                )
                OutlinedTextField(
                    value = password,
                    onValueChange = { password = it },
                    label = { Text("Password") },
                    singleLine = true,
                    visualTransformation = PasswordVisualTransformation(),
                )
                if (!error.isNullOrBlank()) {
                    Text(text = error, color = SquadAirDangerRed, fontSize = 12.sp)
                }
            }
        },
        confirmButton = {
            Button(
                onClick = { onConfirm(password) },
                enabled = password.isNotBlank(),
            ) {
                Text("Join")
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) {
                Text("Cancel")
            }
        },
    )
}
