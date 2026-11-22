package app.squadair.mobile.ui.theme

import android.graphics.Bitmap
import androidx.compose.foundation.Image
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import java.nio.ByteBuffer

val SquadAirBackground = Color(0xFF0E141B)
val SquadAirSurfaceCard = Color(0xFF161E29)
val SquadAirSurfaceElevated = Color(0xFF1E2736)
val SquadAirPrimary = Color(0xFFA8C7FA)
val SquadAirOnPrimary = Color(0xFF062E6F)
val SquadAirSpeakingGreen = Color(0xFF4ADE80)
val SquadAirRelayYellow = Color(0xFFFACC15)
val SquadAirDangerRed = Color(0xFFF87171)
val SquadAirMeshCyan = Color(0xFF38BDF8)
val SquadAirTextPrimary = Color(0xFFF2F5F8)
val SquadAirTextMuted = Color(0xFF94A3B8)
val SquadAirOutline = Color(0xFF2B3648)

private val SquadAirColorScheme = darkColorScheme(
    primary = SquadAirPrimary,
    onPrimary = SquadAirOnPrimary,
    primaryContainer = Color(0xFF1E3A63),
    onPrimaryContainer = SquadAirPrimary,
    secondary = SquadAirMeshCyan,
    onSecondary = SquadAirOnPrimary,
    secondaryContainer = SquadAirSurfaceElevated,
    onSecondaryContainer = SquadAirTextPrimary,
    background = SquadAirBackground,
    onBackground = SquadAirTextPrimary,
    surface = SquadAirSurfaceCard,
    onSurface = SquadAirTextPrimary,
    surfaceVariant = SquadAirSurfaceElevated,
    onSurfaceVariant = SquadAirTextMuted,
    outline = SquadAirOutline,
    error = SquadAirDangerRed,
    onError = Color(0xFF3F0D12),
)

@Composable
fun SquadAirTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = SquadAirColorScheme,
        content = content,
    )
}

@Composable
fun RgbaAvatarImage(
    rgbaBytes: ByteArray?,
    sizeDp: Dp,
    isSpeaking: Boolean = false,
    modifier: Modifier = Modifier,
) {
    val imageBitmap: ImageBitmap? = remember(rgbaBytes) {
        if (rgbaBytes != null && rgbaBytes.size >= 128 * 128 * 4) {
            val bmp = Bitmap.createBitmap(128, 128, Bitmap.Config.ARGB_8888)
            bmp.copyPixelsFromBuffer(ByteBuffer.wrap(rgbaBytes, 0, 128 * 128 * 4))
            bmp.asImageBitmap()
        } else {
            null
        }
    }
    val ringColor = if (isSpeaking) SquadAirSpeakingGreen else SquadAirOutline
    val ringWidth = if (isSpeaking) 2.5.dp else 1.dp

    Box(
        modifier = modifier
            .size(sizeDp)
            .clip(CircleShape)
            .border(ringWidth, ringColor, CircleShape),
    ) {
        if (imageBitmap != null) {
            Image(
                bitmap = imageBitmap,
                contentDescription = "Avatar",
                contentScale = ContentScale.Crop,
                modifier = Modifier.size(sizeDp),
            )
        }
    }
}
