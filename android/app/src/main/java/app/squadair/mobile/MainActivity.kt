package app.squadair.mobile

import android.Manifest
import android.app.PictureInPictureParams
import android.content.Intent
import android.content.pm.ActivityInfo
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.os.Build
import android.os.Bundle
import android.util.Rational
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.core.content.ContextCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.squadair.mobile.core.SquadSessionRepository
import app.squadair.mobile.ui.IncomingCallDialog
import app.squadair.mobile.ui.LobbyScreen
import app.squadair.mobile.ui.RoomScreen
import app.squadair.mobile.ui.StreamStageView
import app.squadair.mobile.ui.theme.SquadAirBackground
import app.squadair.mobile.ui.theme.SquadAirTheme

class MainActivity : ComponentActivity() {
    private lateinit var repository: SquadSessionRepository

    private val permissionLauncher = registerForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) { /* Permissions state is checked dynamically before audio/notification actions */ }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        repository = SquadSessionRepository.getInstance(this)
        requestNecessaryPermissions()
        handleIncomingIntent(intent)

        setContent {
            SquadAirTheme {
                val state by repository.uiState.collectAsStateWithLifecycle()
                val telemetry by repository.telemetry.collectAsStateWithLifecycle()

                LaunchedEffect(state.isFullscreenStream) {
                    applyFullscreenSystemBars(state.isFullscreenStream)
                }

                Surface(
                    modifier = Modifier.fillMaxSize(),
                    color = SquadAirBackground,
                ) {
                    if (state.isInPipMode && state.inRoom && state.watchedPeerId != null) {
                        val watchedPeer = state.peers.firstOrNull { it.peerId == state.watchedPeerId }
                        Box(modifier = Modifier.fillMaxSize()) {
                            StreamStageView(
                                peer = watchedPeer,
                                streamVolume = state.profile.streamVolume,
                                isFullscreen = true,
                                isPipMode = true,
                                onToggleFullscreen = {},
                                onCloseStream = { repository.watchPeerStream(null) },
                                onStreamVolumeChange = { repository.setStreamVolume(it) },
                            )
                        }
                    } else if (state.inRoom) {
                        RoomScreen(
                            state = state,
                            telemetry = telemetry,
                            repository = repository,
                        )
                    } else {
                        LobbyScreen(
                            state = state,
                            repository = repository,
                        )
                    }

                    state.incomingCall?.let { call ->
                        IncomingCallDialog(
                            call = call,
                            onAccept = { repository.acceptIncomingCall(call.inviteUri) },
                            onDecline = { repository.declineIncomingCall() },
                        )
                    }
                }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleIncomingIntent(intent)
    }

    override fun onUserLeaveHint() {
        super.onUserLeaveHint()
        val state = repository.uiState.value
        if (state.inRoom && state.watchedPeerId != null) {
            val params = PictureInPictureParams.Builder()
                .setAspectRatio(Rational(16, 9))
                .build()
            enterPictureInPictureMode(params)
        }
    }

    override fun onPictureInPictureModeChanged(
        isInPictureInPictureMode: Boolean,
        newConfig: Configuration,
    ) {
        super.onPictureInPictureModeChanged(isInPictureInPictureMode, newConfig)
        repository.setPipMode(isInPictureInPictureMode)
    }

    private fun handleIncomingIntent(intent: Intent?) {
        val data = intent?.data ?: return
        if (data.scheme == "squad" || data.scheme == "voice") {
            repository.joinRoomByInvite(data.toString())
        }
    }

    private fun applyFullscreenSystemBars(fullscreen: Boolean) {
        val controller = WindowCompat.getInsetsController(window, window.decorView)
        controller.systemBarsBehavior =
            WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        if (fullscreen) {
            requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
            controller.hide(WindowInsetsCompat.Type.systemBars())
        } else {
            requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
            controller.show(WindowInsetsCompat.Type.systemBars())
        }
    }

    private fun requestNecessaryPermissions() {
        val perms = mutableListOf(Manifest.permission.RECORD_AUDIO)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            perms.add(Manifest.permission.POST_NOTIFICATIONS)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            perms.add(Manifest.permission.BLUETOOTH_CONNECT)
        }
        val missing = perms.filter {
            ContextCompat.checkSelfPermission(this, it) != PackageManager.PERMISSION_GRANTED
        }
        if (missing.isNotEmpty()) {
            permissionLauncher.launch(missing.toTypedArray())
        }
    }
}
