package app.squadair.mobile.ui

import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Fullscreen
import androidx.compose.material.icons.filled.FullscreenExit
import androidx.compose.material.icons.filled.VolumeUp
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import app.squadair.mobile.core.SquadNativeBridge
import app.squadair.mobile.ui.theme.SquadAirDangerRed
import app.squadair.mobile.ui.theme.SquadAirPrimary
import app.squadair.mobile.ui.theme.SquadAirRelayYellow
import app.squadair.mobile.ui.theme.SquadAirSurfaceCard
import app.squadair.mobile.ui.theme.SquadAirTextPrimary
import uniffi.squad_core.MobilePeerSnapshot

@Composable
fun StreamStageView(
    peer: MobilePeerSnapshot?,
    streamVolume: Float,
    isFullscreen: Boolean,
    isPipMode: Boolean,
    onToggleFullscreen: () -> Unit,
    onCloseStream: () -> Unit,
    onStreamVolumeChange: (Float) -> Unit,
    modifier: Modifier = Modifier,
) {
    var scale by remember { mutableFloatStateOf(1f) }
    var offset by remember { mutableStateOf(Offset.Zero) }

    DisposableEffect(Unit) {
        onDispose {
            SquadNativeBridge.nativeDetachSurface()
        }
    }

    val containerModifier = if (isFullscreen || isPipMode) {
        modifier
            .fillMaxSize()
            .background(Color.Black)
    } else {
        modifier
            .fillMaxWidth()
            .aspectRatio(16f / 9f)
            .background(Color.Black)
    }

    Box(
        modifier = containerModifier
            .clipToBounds()
            .pointerInput(isPipMode) {
                if (!isPipMode) {
                    detectTransformGestures { _, pan, zoom, _ ->
                        val nextScale = (scale * zoom).coerceIn(1f, 4f)
                        scale = nextScale
                        if (nextScale <= 1.01f) {
                            offset = Offset.Zero
                        } else {
                            val maxPanX = (size.width * (nextScale - 1f)) * 0.5f
                            val maxPanY = (size.height * (nextScale - 1f)) * 0.5f
                            offset = Offset(
                                x = (offset.x + pan.x).coerceIn(-maxPanX, maxPanX),
                                y = (offset.y + pan.y).coerceIn(-maxPanY, maxPanY),
                            )
                        }
                    }
                }
            }
            .pointerInput(isPipMode) {
                if (!isPipMode) {
                    detectTapGestures(
                        onDoubleTap = {
                            scale = 1f
                            offset = Offset.Zero
                        },
                    )
                }
            },
    ) {
        AndroidView(
            factory = { ctx ->
                SurfaceView(ctx).apply {
                    holder.addCallback(
                        object : SurfaceHolder.Callback {
                            override fun surfaceCreated(holder: SurfaceHolder) {
                                SquadNativeBridge.nativeAttachSurface(holder.surface)
                            }

                            override fun surfaceChanged(
                                holder: SurfaceHolder,
                                format: Int,
                                width: Int,
                                height: Int,
                            ) {
                                SquadNativeBridge.nativeAttachSurface(holder.surface)
                            }

                            override fun surfaceDestroyed(holder: SurfaceHolder) {
                                SquadNativeBridge.nativeDetachSurface()
                            }
                        },
                    )
                }
            },
            modifier = Modifier
                .fillMaxSize()
                .graphicsLayer(
                    scaleX = scale,
                    scaleY = scale,
                    translationX = offset.x,
                    translationY = offset.y,
                ),
        )

        if (!isPipMode) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .align(Alignment.TopCenter)
                    .background(Color.Black.copy(alpha = 0.55f))
                    .padding(horizontal = 10.dp, vertical = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Surface(
                    color = SquadAirDangerRed,
                    shape = RoundedCornerShape(4.dp),
                ) {
                    Text(
                        text = "LIVE",
                        color = Color.Black,
                        fontSize = 10.sp,
                        fontWeight = FontWeight.Bold,
                        modifier = Modifier.padding(horizontal = 5.dp, vertical = 2.dp),
                    )
                }
                Spacer(modifier = Modifier.width(8.dp))
                val info = peer?.streamInfo
                val title = buildString {
                    append(peer?.nickname ?: "Stream")
                    if (info != null) {
                        append(" • ${info.width}x${info.height}@${info.fps}")
                    }
                }
                Text(
                    text = title,
                    color = SquadAirTextPrimary,
                    fontSize = 12.sp,
                    fontWeight = FontWeight.Medium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                IconButton(
                    onClick = onToggleFullscreen,
                    modifier = Modifier.size(32.dp),
                ) {
                    Icon(
                        imageVector = if (isFullscreen) Icons.Default.FullscreenExit else Icons.Default.Fullscreen,
                        contentDescription = "Toggle fullscreen",
                        tint = SquadAirTextPrimary,
                    )
                }
                IconButton(
                    onClick = onCloseStream,
                    modifier = Modifier.size(32.dp),
                ) {
                    Icon(
                        imageVector = Icons.Default.Close,
                        contentDescription = "Close stream",
                        tint = SquadAirTextPrimary,
                    )
                }
            }

            if (peer?.route == "RELAY") {
                Surface(
                    color = SquadAirSurfaceCard.copy(alpha = 0.92f),
                    shape = RoundedCornerShape(10.dp),
                    modifier = Modifier
                        .align(Alignment.Center)
                        .padding(16.dp),
                ) {
                    Row(
                        modifier = Modifier.padding(12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Icon(
                            imageVector = Icons.Default.Warning,
                            contentDescription = null,
                            tint = SquadAirRelayYellow,
                        )
                        Spacer(modifier = Modifier.width(10.dp))
                        Text(
                            text = "Voice & Chat active via CGNAT Server Relay. Direct P2P or Mesh Relay peer is required for 1080p60 video.",
                            color = SquadAirTextPrimary,
                            fontSize = 12.sp,
                        )
                    }
                }
            }

            if (peer?.streamInfo?.hasAudio == true) {
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .align(Alignment.BottomCenter)
                        .background(Color.Black.copy(alpha = 0.55f))
                        .padding(horizontal = 12.dp, vertical = 2.dp),
                ) {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Icon(
                            imageVector = Icons.Default.VolumeUp,
                            contentDescription = "Stream volume",
                            tint = SquadAirPrimary,
                            modifier = Modifier.size(18.dp),
                        )
                        Slider(
                            value = streamVolume.coerceIn(0f, 2f),
                            onValueChange = onStreamVolumeChange,
                            valueRange = 0f..2f,
                            colors = SliderDefaults.colors(
                                thumbColor = SquadAirPrimary,
                                activeTrackColor = SquadAirPrimary,
                            ),
                            modifier = Modifier.weight(1f),
                        )
                        Text(
                            text = "${(streamVolume * 100).toInt()}%",
                            color = SquadAirTextPrimary,
                            fontSize = 11.sp,
                        )
                    }
                }
            }
        }
    }
}
