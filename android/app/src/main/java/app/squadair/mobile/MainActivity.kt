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
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.core.content.ContextCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.squadair.mobile.core.BatterySurvivalHelper
import app.squadair.mobile.core.SquadSessionRepository
import app.squadair.mobile.service.IncomingCallNotificationManager
import app.squadair.mobile.ui.BatteryOptimizationDialog
import app.squadair.mobile.ui.IncomingCallDialog
import app.squadair.mobile.ui.LobbyScreen
import app.squadair.mobile.ui.RoomScreen
import app.squadair.mobile.ui.StreamStageView
import app.squadair.mobile.ui.theme.SquadAirBackground
import app.squadair.mobile.ui.theme.SquadAirTheme

class MainActivity : ComponentActivity() {
    private lateinit var repository: SquadSessionRepository
    private var showBatteryOnboarding by mutableStateOf(false)

    private val permissionLauncher = registerForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) {
        maybeTriggerBatteryOnboarding()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge(
            statusBarStyle = SystemBarStyle.dark(android.graphics.Color.TRANSPARENT),
            navigationBarStyle = SystemBarStyle.dark(android.graphics.Color.TRANSPARENT),
        )
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

                    if (showBatteryOnboarding) {
                        BatteryOptimizationDialog(
                            onDismiss = { showBatteryOnboarding = false },
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

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) {
            repository.onUiVisibilityPossiblyChanged()
        }
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
        if (intent == null) return
        when (intent.action) {
            IncomingCallNotificationManager.ACTION_ACCEPT_INCOMING_CALL -> {
                val inviteUri = intent.getStringExtra(IncomingCallNotificationManager.EXTRA_INVITE_URI)
                intent.action = null
                intent.removeExtra(IncomingCallNotificationManager.EXTRA_INVITE_URI)
                if (!inviteUri.isNullOrBlank()) {
                    repository.acceptIncomingCall(inviteUri)
                }
                return
            }
            IncomingCallNotificationManager.ACTION_SHOW_INCOMING_CALL -> {
                intent.action = null
                return
            }
        }
        val data = intent.data ?: return
        if (data.scheme == "squad" || data.scheme == "voice") {
            repository.joinRoomByInvite(data.toString())
        }
    }

    private fun maybeTriggerBatteryOnboarding() {
        if (BatterySurvivalHelper.shouldPromptOnboarding(this)) {
            BatterySurvivalHelper.markOnboardingShown(this)
            showBatteryOnboarding = true
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
        val missing = perms.filter {
            ContextCompat.checkSelfPermission(this, it) != PackageManager.PERMISSION_GRANTED
        }
        if (missing.isNotEmpty()) {
            permissionLauncher.launch(missing.toTypedArray())
        } else {
            maybeTriggerBatteryOnboarding()
        }
    }
}
