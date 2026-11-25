package app.squadair.mobile.ui

import android.graphics.Bitmap
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.squadair.mobile.BuildConfig
import app.squadair.mobile.core.AppUpdater
import app.squadair.mobile.core.BatterySurvivalHelper
import app.squadair.mobile.core.SquadSessionRepository
import app.squadair.mobile.core.UpdateState
import app.squadair.mobile.ui.theme.RgbaAvatarImage
import app.squadair.mobile.ui.theme.SquadAirDangerRed
import app.squadair.mobile.ui.theme.SquadAirOnPrimary
import app.squadair.mobile.ui.theme.SquadAirPrimary
import app.squadair.mobile.ui.theme.SquadAirRelayYellow
import app.squadair.mobile.ui.theme.SquadAirSpeakingGreen
import app.squadair.mobile.ui.theme.SquadAirSurfaceCard
import app.squadair.mobile.ui.theme.SquadAirSurfaceElevated
import app.squadair.mobile.ui.theme.SquadAirTextMuted
import app.squadair.mobile.ui.theme.SquadAirTextPrimary
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import uniffi.squad_core.MobileProfileSnapshot
import java.io.ByteArrayOutputStream

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsSheet(
    profile: MobileProfileSnapshot,
    selfAvatarRgba: ByteArray?,
    repository: SquadSessionRepository,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val scope = rememberCoroutineScope()
    val uiState by repository.uiState.collectAsStateWithLifecycle()
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    var nickname by remember(profile.nickname) { mutableStateOf(profile.nickname) }
    var server by remember(profile.rendezvousServer) { mutableStateOf(profile.rendezvousServer) }
    var inputGain by remember(profile.inputGain) { mutableFloatStateOf(profile.inputGain) }
    var vadDb by remember(profile.vadThresholdDb) { mutableFloatStateOf(profile.vadThresholdDb) }
    var showBatteryDialog by remember { mutableStateOf(false) }
    var isIgnoringBatteryOpt by remember {
        mutableStateOf(BatterySurvivalHelper.isIgnoringBatteryOptimizations(context))
    }

    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) {
                isIgnoringBatteryOpt = BatterySurvivalHelper.isIgnoringBatteryOptimizations(context)
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose {
            lifecycleOwner.lifecycle.removeObserver(observer)
        }
    }

    val installPermLauncher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.StartActivityForResult(),
    ) {
        repository.onInstallPermissionResult()
    }

    val avatarPicker = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.PickVisualMedia(),
    ) { uri: Uri? ->
        if (uri != null) {
            scope.launch(Dispatchers.IO) {
                val raw = context.contentResolver.openInputStream(uri)?.use { it.readBytes() }
                    ?: return@launch
                val decoded = decodeBoundedBitmap(raw, maxDim = 512) ?: return@launch
                val maxSide = 512
                val ratio = minOf(maxSide.toFloat() / decoded.width, maxSide.toFloat() / decoded.height, 1f)
                val scaled = if (ratio < 1f) {
                    Bitmap.createScaledBitmap(
                        decoded,
                        (decoded.width * ratio).toInt().coerceAtLeast(1),
                        (decoded.height * ratio).toInt().coerceAtLeast(1),
                        true,
                    )
                } else {
                    decoded
                }
                val out = ByteArrayOutputStream()
                if (scaled.compress(Bitmap.CompressFormat.PNG, 100, out)) {
                    repository.setCustomAvatar(out.toByteArray())
                }
            }
        }
    }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        containerColor = SquadAirSurfaceCard,
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .navigationBarsPadding()
                .imePadding()
                .padding(horizontal = 20.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                text = "Voice & Profile Settings",
                color = SquadAirTextPrimary,
                fontSize = 18.sp,
                fontWeight = FontWeight.SemiBold,
            )

            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                RgbaAvatarImage(rgbaBytes = selfAvatarRgba, sizeDp = 56.dp)
                FilledTonalButton(
                    onClick = {
                        avatarPicker.launch(
                            PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly),
                        )
                    },
                ) {
                    Text("Change Avatar")
                }
                if (profile.hasCustomAvatar) {
                    TextButton(onClick = { repository.clearCustomAvatar() }) {
                        Text("Reset", color = SquadAirTextMuted)
                    }
                }
            }

            OutlinedTextField(
                value = nickname,
                onValueChange = { nickname = it },
                label = { Text("Nickname") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )

            OutlinedTextField(
                value = server,
                onValueChange = { server = it },
                label = { Text("Rendezvous Server") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )

            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Column(modifier = Modifier.weight(1f)) {
                    Text("Echo Cancellation (AEC3)", color = SquadAirTextPrimary, fontSize = 14.sp)
                    Text("Reduces speaker feedback during loudspeaker calls", color = SquadAirTextMuted, fontSize = 11.sp)
                    if (uiState.isSpeakerAecOverride) {
                        Text(
                            text = "Auto-enabled for Speaker (restores previous setting on switch)",
                            color = SquadAirPrimary,
                            fontSize = 11.sp,
                        )
                    }
                }
                Switch(
                    checked = profile.aecEnabled,
                    onCheckedChange = { repository.setAecEnabled(it) },
                )
            }

            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Column(modifier = Modifier.weight(1f)) {
                    Text("RNNoise Suppression", color = SquadAirTextPrimary, fontSize = 14.sp)
                    Text("Pure Rust real-time background noise reduction", color = SquadAirTextMuted, fontSize = 11.sp)
                }
                Switch(
                    checked = profile.nsMode != "Off",
                    onCheckedChange = { repository.setRnnoiseEnabled(it) },
                )
            }

            Column {
                Text(
                    text = "Input Gain: ${(inputGain * 100).toInt()}%",
                    color = SquadAirTextPrimary,
                    fontSize = 13.sp,
                )
                Slider(
                    value = inputGain.coerceIn(0f, 2.5f),
                    onValueChange = {
                        inputGain = it
                        repository.setInputGain(it)
                    },
                    valueRange = 0f..2.5f,
                    colors = SliderDefaults.colors(
                        thumbColor = SquadAirPrimary,
                        activeTrackColor = SquadAirPrimary,
                    ),
                )
            }

            Column {
                Text(
                    text = "VAD Gate Threshold: ${vadDb.toInt()} dB",
                    color = SquadAirTextPrimary,
                    fontSize = 13.sp,
                )
                Slider(
                    value = vadDb.coerceIn(-60f, 0f),
                    onValueChange = {
                        vadDb = it
                        repository.setVadThresholdDb(it)
                    },
                    valueRange = -60f..0f,
                    colors = SliderDefaults.colors(
                        thumbColor = SquadAirPrimary,
                        activeTrackColor = SquadAirPrimary,
                    ),
                )
            }

            Card(
                colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceElevated),
                shape = RoundedCornerShape(12.dp),
            ) {
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.SpaceBetween,
                ) {
                    Column(modifier = Modifier.weight(1f)) {
                        Text(
                            text = "Background & Battery Survival",
                            color = SquadAirTextPrimary,
                            fontSize = 14.sp,
                            fontWeight = FontWeight.Medium,
                        )
                        Text(
                            text = if (isIgnoringBatteryOpt) "Unrestricted" else "Restricted (may sleep in background)",
                            color = if (isIgnoringBatteryOpt) SquadAirSpeakingGreen else SquadAirRelayYellow,
                            fontSize = 11.sp,
                        )
                    }
                    FilledTonalButton(onClick = { showBatteryDialog = true }) {
                        Text("Configure")
                    }
                }
            }

            Card(
                colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceElevated),
                shape = RoundedCornerShape(12.dp),
            ) {
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(12.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.SpaceBetween,
                    ) {
                        Column(modifier = Modifier.weight(1f)) {
                            Text(
                                text = "App Updates",
                                color = SquadAirTextPrimary,
                                fontSize = 14.sp,
                                fontWeight = FontWeight.Medium,
                            )
                            Text(
                                text = "Version ${BuildConfig.VERSION_NAME}",
                                color = SquadAirTextMuted,
                                fontSize = 11.sp,
                            )
                        }
                        FilledTonalButton(
                            onClick = { repository.checkForUpdates(silent = false) },
                            enabled = uiState.updateState !is UpdateState.Checking &&
                                uiState.updateState !is UpdateState.Downloading,
                        ) {
                            Text("Check for Updates")
                        }
                    }

                    when (val upd = uiState.updateState) {
                        UpdateState.Idle -> Unit
                        UpdateState.Checking -> {
                            Text("Checking GitHub releases...", color = SquadAirTextMuted, fontSize = 12.sp)
                        }
                        is UpdateState.UpToDate -> {
                            Text(
                                text = "Up to date (${upd.currentVersion})",
                                color = SquadAirSpeakingGreen,
                                fontSize = 12.sp,
                            )
                        }
                        is UpdateState.Available -> {
                            Text(
                                text = "Update ${upd.release.tagName} available",
                                color = SquadAirPrimary,
                                fontSize = 12.sp,
                            )
                            Button(
                                onClick = { repository.downloadAndInstallUpdate(upd.release) },
                                modifier = Modifier.fillMaxWidth(),
                            ) {
                                Text("Download & Install")
                            }
                        }
                        is UpdateState.Downloading -> {
                            val pct = upd.progress?.let { "${(it * 100).toInt()}%" } ?: "..."
                            Text(
                                text = "Downloading ${upd.release.tagName} ($pct)",
                                color = SquadAirTextPrimary,
                                fontSize = 12.sp,
                            )
                            val prog = upd.progress
                            if (prog != null) {
                                LinearProgressIndicator(
                                    progress = { prog },
                                    modifier = Modifier.fillMaxWidth(),
                                )
                            } else {
                                LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
                            }
                        }
                        is UpdateState.ReadyToInstall -> {
                            Button(
                                onClick = { repository.installDownloadedUpdate() },
                                modifier = Modifier.fillMaxWidth(),
                            ) {
                                Text("Install Update")
                            }
                        }
                        is UpdateState.NeedsInstallPermission -> {
                            Text(
                                text = "Grant install permission for SquadAir to apply ${upd.release.tagName}",
                                color = SquadAirRelayYellow,
                                fontSize = 12.sp,
                            )
                            Button(
                                onClick = {
                                    installPermLauncher.launch(
                                        AppUpdater.createUnknownSourcesSettingsIntent(context),
                                    )
                                },
                                modifier = Modifier.fillMaxWidth(),
                            ) {
                                Text("Allow & Install")
                            }
                        }
                        is UpdateState.Error -> {
                            Text(
                                text = upd.message,
                                color = SquadAirDangerRed,
                                fontSize = 12.sp,
                            )
                        }
                    }
                }
            }

            Button(
                onClick = {
                    if (nickname.isNotBlank() && nickname != profile.nickname) {
                        repository.setNickname(nickname)
                    }
                    if (server.isNotBlank() && server != profile.rendezvousServer) {
                        repository.setRendezvousServer(server)
                    }
                    onDismiss()
                },
                colors = ButtonDefaults.buttonColors(
                    containerColor = SquadAirPrimary,
                    contentColor = SquadAirOnPrimary,
                ),
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text("Save & Close")
            }

            Spacer(modifier = Modifier.height(12.dp))
        }
    }

    if (showBatteryDialog) {
        BatteryOptimizationDialog(
            onDismiss = {
                isIgnoringBatteryOpt = BatterySurvivalHelper.isIgnoringBatteryOptimizations(context)
                showBatteryDialog = false
            },
        )
    }
}
