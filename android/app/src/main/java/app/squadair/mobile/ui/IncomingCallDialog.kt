package app.squadair.mobile.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Call
import androidx.compose.material.icons.filled.CallEnd
import androidx.compose.material.icons.filled.VerifiedUser
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.squadair.mobile.ui.theme.SquadAirDangerRed
import app.squadair.mobile.ui.theme.SquadAirMeshCyan
import app.squadair.mobile.ui.theme.SquadAirOnPrimary
import app.squadair.mobile.ui.theme.SquadAirSpeakingGreen
import app.squadair.mobile.ui.theme.SquadAirSurfaceCard
import app.squadair.mobile.ui.theme.SquadAirSurfaceElevated
import app.squadair.mobile.ui.theme.SquadAirTextMuted
import app.squadair.mobile.ui.theme.SquadAirTextPrimary
import uniffi.squad_core.MobileIncomingCall

@Composable
fun IncomingCallDialog(
    call: MobileIncomingCall,
    onAccept: () -> Unit,
    onDecline: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDecline,
        containerColor = SquadAirSurfaceCard,
        title = {
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Surface(
                    color = SquadAirSurfaceElevated,
                    shape = RoundedCornerShape(8.dp),
                ) {
                    Row(
                        modifier = Modifier.padding(horizontal = 8.dp, vertical = 4.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Icon(
                            imageVector = Icons.Default.VerifiedUser,
                            contentDescription = null,
                            tint = if (call.isKnownFriend) SquadAirSpeakingGreen else SquadAirMeshCyan,
                        )
                        Spacer(modifier = Modifier.width(6.dp))
                        val badgeText = buildString {
                            append(if (call.isKnownFriend) "Verified Friend" else "X25519 Call")
                            if (call.isPartyInvite) append(" • Party Invite")
                        }
                        Text(
                            text = badgeText,
                            color = if (call.isKnownFriend) SquadAirSpeakingGreen else SquadAirMeshCyan,
                            fontSize = 12.sp,
                            fontWeight = FontWeight.Medium,
                        )
                    }
                }
                Text(
                    text = call.senderNickname,
                    color = SquadAirTextPrimary,
                    fontSize = 20.sp,
                    fontWeight = FontWeight.Bold,
                )
            }
        },
        text = {
            Text(
                text = if (call.isPartyInvite) {
                    "Inviting you to join their active SquadAir voice room."
                } else {
                    "Incoming encrypted SquadAir voice call."
                },
                color = SquadAirTextMuted,
                fontSize = 14.sp,
            )
        },
        confirmButton = {
            Button(
                onClick = onAccept,
                colors = ButtonDefaults.buttonColors(
                    containerColor = SquadAirSpeakingGreen,
                    contentColor = SquadAirOnPrimary,
                ),
            ) {
                Icon(Icons.Default.Call, contentDescription = null)
                Spacer(modifier = Modifier.width(6.dp))
                Text("Accept & Join", fontWeight = FontWeight.SemiBold)
            }
        },
        dismissButton = {
            TextButton(
                onClick = onDecline,
                colors = ButtonDefaults.textButtonColors(contentColor = SquadAirDangerRed),
                modifier = Modifier.fillMaxWidth(0.45f),
            ) {
                Icon(Icons.Default.CallEnd, contentDescription = null)
                Spacer(modifier = Modifier.width(6.dp))
                Text("Decline")
            }
        },
    )
}
