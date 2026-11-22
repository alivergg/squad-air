package app.squadair.mobile.service

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import app.squadair.mobile.MainActivity
import app.squadair.mobile.R
import app.squadair.mobile.core.SquadSessionRepository

class VoiceCallForegroundService : Service() {
    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()
        acquireLocks()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_TOGGLE_MUTE -> {
                SquadSessionRepository.getInstance(this).toggleMicMute()
                startOrUpdateForeground()
            }
            ACTION_DISCONNECT -> {
                SquadSessionRepository.getInstance(this).leaveRoom()
                stopSelf()
            }
            ACTION_UPDATE -> {
                startOrUpdateForeground()
            }
            else -> {
                startOrUpdateForeground()
            }
        }
        return START_STICKY
    }

    override fun onDestroy() {
        releaseLocks()
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }

    private fun startOrUpdateForeground() {
        val notification = buildNotification()
        val hasMicPermission = ContextCompat.checkSelfPermission(
            this,
            Manifest.permission.RECORD_AUDIO,
        ) == PackageManager.PERMISSION_GRANTED

        val serviceType = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            if (hasMicPermission) {
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE or
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK
            } else {
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK
            }
        } else {
            0
        }

        try {
            ServiceCompat.startForeground(this, NOTIFICATION_ID, notification, serviceType)
        } catch (_: SecurityException) {
            val nm = getSystemService(NotificationManager::class.java)
            nm?.notify(NOTIFICATION_ID, notification)
        }
    }

    private fun buildNotification(): Notification {
        val repo = SquadSessionRepository.getInstance(this)
        val state = repo.uiState.value
        val peerCount = state.peers.size
        val peerLabel = if (peerCount == 1) "1 peer" else "$peerCount peers"
        val title = "SquadAir • Connected ($peerLabel)"

        val micStatus = when {
            state.profile.deafened -> "Deafened"
            state.profile.micMuted -> "Mic Muted"
            else -> "Mic Live"
        }
        val avgRtt = state.peers
            .map { it.rttMs.toInt() }
            .filter { it > 0 }
            .takeIf { it.isNotEmpty() }
            ?.average()
            ?.toInt()
        val subtitle = if (avgRtt != null) {
            "$micStatus • ${avgRtt} ms RTT"
        } else {
            micStatus
        }

        val openAppIntent = Intent(this, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
        }
        val openPending = PendingIntent.getActivity(
            this,
            0,
            openAppIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

        val muteIntent = Intent(this, VoiceCallForegroundService::class.java).apply {
            action = ACTION_TOGGLE_MUTE
        }
        val mutePending = PendingIntent.getService(
            this,
            1,
            muteIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val muteLabel = if (state.profile.micMuted) "Unmute Mic" else "Mute Mic"

        val disconnectIntent = Intent(this, VoiceCallForegroundService::class.java).apply {
            action = ACTION_DISCONNECT
        }
        val disconnectPending = PendingIntent.getService(
            this,
            2,
            disconnectIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_squadair)
            .setContentTitle(title)
            .setContentText(subtitle)
            .setContentIntent(openPending)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .addAction(0, muteLabel, mutePending)
            .addAction(0, "Disconnect", disconnectPending)
            .build()
    }

    private fun createNotificationChannel() {
        val nm = getSystemService(NotificationManager::class.java) ?: return
        val channel = NotificationChannel(
            CHANNEL_ID,
            "Active Voice Room",
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = "Keeps encrypted P2P voice active while the app is in background"
            setShowBadge(false)
        }
        nm.createNotificationChannel(channel)
    }

    private fun acquireLocks() {
        val pm = getSystemService(Context.POWER_SERVICE) as? PowerManager
        if (pm != null && wakeLock == null) {
            wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, WAKE_LOCK_TAG).apply {
                setReferenceCounted(false)
                acquire()
            }
        }

        val wm = applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
        if (wm != null && wifiLock == null) {
            val mode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                WifiManager.WIFI_MODE_FULL_LOW_LATENCY
            } else {
                @Suppress("DEPRECATION")
                WifiManager.WIFI_MODE_FULL_HIGH_PERF
            }
            wifiLock = wm.createWifiLock(mode, WIFI_LOCK_TAG).apply {
                setReferenceCounted(false)
                acquire()
            }
        }
    }

    private fun releaseLocks() {
        try {
            if (wakeLock?.isHeld == true) {
                wakeLock?.release()
            }
        } catch (_: Throwable) {
        }
        wakeLock = null

        try {
            if (wifiLock?.isHeld == true) {
                wifiLock?.release()
            }
        } catch (_: Throwable) {
        }
        wifiLock = null
    }

    companion object {
        private const val CHANNEL_ID = "squadair_voice_call"
        private const val NOTIFICATION_ID = 3478
        private const val WAKE_LOCK_TAG = "SquadAir::VoiceCallWakeLock"
        private const val WIFI_LOCK_TAG = "SquadAir::VoiceWifiLock"
        private const val MULTICAST_TAG = "SquadAir::LanDiscoveryMulticast"

        const val ACTION_START = "app.squadair.mobile.action.START_CALL"
        const val ACTION_UPDATE = "app.squadair.mobile.action.UPDATE_CALL"
        const val ACTION_TOGGLE_MUTE = "app.squadair.mobile.action.TOGGLE_MUTE"
        const val ACTION_DISCONNECT = "app.squadair.mobile.action.DISCONNECT"

        @Volatile
        private var isRunning = false

        @Volatile
        private var lobbyMulticastLock: WifiManager.MulticastLock? = null

        fun start(context: Context) {
            isRunning = true
            val intent = Intent(context, VoiceCallForegroundService::class.java).apply {
                action = ACTION_START
            }
            ContextCompat.startForegroundService(context, intent)
        }

        fun updateNotificationIfRunning(context: Context) {
            if (!isRunning) return
            val intent = Intent(context, VoiceCallForegroundService::class.java).apply {
                action = ACTION_UPDATE
            }
            try {
                context.startService(intent)
            } catch (_: IllegalStateException) {
            }
        }

        fun stop(context: Context) {
            isRunning = false
            context.stopService(Intent(context, VoiceCallForegroundService::class.java))
        }

        @Synchronized
        fun setLobbyMulticastLock(context: Context, active: Boolean) {
            val wm = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
                ?: return
            if (active) {
                if (lobbyMulticastLock == null) {
                    lobbyMulticastLock = wm.createMulticastLock(MULTICAST_TAG).apply {
                        setReferenceCounted(false)
                    }
                }
                if (lobbyMulticastLock?.isHeld == false) {
                    try {
                        lobbyMulticastLock?.acquire()
                    } catch (_: Throwable) {
                    }
                }
            } else {
                try {
                    if (lobbyMulticastLock?.isHeld == true) {
                        lobbyMulticastLock?.release()
                    }
                } catch (_: Throwable) {
                }
            }
        }
    }
}
