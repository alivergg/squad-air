package app.squadair.mobile.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import app.squadair.mobile.core.BatterySurvivalHelper
import app.squadair.mobile.core.OemFamily
import app.squadair.mobile.ui.theme.SquadAirOnPrimary
import app.squadair.mobile.ui.theme.SquadAirPrimary
import app.squadair.mobile.ui.theme.SquadAirRelayYellow
import app.squadair.mobile.ui.theme.SquadAirSpeakingGreen
import app.squadair.mobile.ui.theme.SquadAirSurfaceCard
import app.squadair.mobile.ui.theme.SquadAirSurfaceElevated
import app.squadair.mobile.ui.theme.SquadAirTextMuted
import app.squadair.mobile.ui.theme.SquadAirTextPrimary

@Composable
fun BatteryOptimizationDialog(
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val oem = remember { BatterySurvivalHelper.detectOemFamily() }
    var isIgnoring by remember {
        mutableStateOf(BatterySurvivalHelper.isIgnoringBatteryOptimizations(context))
    }

    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) {
                isIgnoring = BatterySurvivalHelper.isIgnoringBatteryOptimizations(context)
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose {
            lifecycleOwner.lifecycle.removeObserver(observer)
        }
    }

    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = SquadAirSurfaceCard,
        title = {
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(
                    text = "Background & Call Reliability",
                    color = SquadAirTextPrimary,
                    fontSize = 18.sp,
                    fontWeight = FontWeight.SemiBold,
                )
                Surface(
                    color = SquadAirSurfaceElevated,
                    shape = RoundedCornerShape(8.dp),
                ) {
                    Text(
                        text = if (isIgnoring) "Status: Unrestricted" else "Status: Battery Restricted",
                        color = if (isIgnoring) SquadAirSpeakingGreen else SquadAirRelayYellow,
                        fontSize = 12.sp,
                        fontWeight = FontWeight.Medium,
                        modifier = Modifier.padding(horizontal = 8.dp, vertical = 4.dp),
                    )
                }
            }
        },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    text = "SquadAir maintains a direct encrypted P2P connection while minimized so friends see you online and incoming calls ring reliably.",
                    color = SquadAirTextMuted,
                    fontSize = 13.sp,
                )
                oem.guidanceSteps.forEach { step ->
                    Text(
                        text = "• $step",
                        color = SquadAirTextPrimary,
                        fontSize = 12.sp,
                    )
                }
                Spacer(modifier = Modifier.height(4.dp))
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(
                        onClick = { BatterySurvivalHelper.requestIgnoreBatteryOptimizations(context) },
                        colors = ButtonDefaults.buttonColors(
                            containerColor = SquadAirPrimary,
                            contentColor = SquadAirOnPrimary,
                        ),
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Text(if (isIgnoring) "Review Battery Exemption" else "Disable Battery Optimization")
                    }
                    FilledTonalButton(
                        onClick = { BatterySurvivalHelper.openOemBackgroundSettings(context) },
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        val buttonText = if (oem == OemFamily.GENERIC) {
                            "Open System Battery Settings"
                        } else {
                            "Open ${oem.displayName} Background Settings"
                        }
                        Text(buttonText)
                    }
                }
            }
        },
        confirmButton = {
            Row {
                TextButton(onClick = onDismiss) {
                    Text(if (isIgnoring) "Done" else "Later", color = SquadAirPrimary)
                }
            }
        },
    )
}
