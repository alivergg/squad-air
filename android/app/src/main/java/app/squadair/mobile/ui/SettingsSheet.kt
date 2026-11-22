package app.squadair.mobile.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.squadair.mobile.core.SquadSessionRepository
import app.squadair.mobile.ui.theme.SquadAirOnPrimary
import app.squadair.mobile.ui.theme.SquadAirPrimary
import app.squadair.mobile.ui.theme.SquadAirSurfaceCard
import app.squadair.mobile.ui.theme.SquadAirTextMuted
import app.squadair.mobile.ui.theme.SquadAirTextPrimary
import uniffi.squad_core.MobileProfileSnapshot

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsSheet(
    profile: MobileProfileSnapshot,
    repository: SquadSessionRepository,
    onDismiss: () -> Unit,
) {
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    var nickname by remember(profile.nickname) { mutableStateOf(profile.nickname) }
    var server by remember(profile.rendezvousServer) { mutableStateOf(profile.rendezvousServer) }
    var inputGain by remember(profile.inputGain) { mutableFloatStateOf(profile.inputGain) }
    var vadDb by remember(profile.vadThresholdDb) { mutableFloatStateOf(profile.vadThresholdDb) }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        containerColor = SquadAirSurfaceCard,
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 20.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                text = "Voice & Profile Settings",
                color = SquadAirTextPrimary,
                fontSize = 18.sp,
                fontWeight = FontWeight.SemiBold,
            )

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
}
