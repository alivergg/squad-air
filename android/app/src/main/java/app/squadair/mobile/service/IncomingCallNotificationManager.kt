package app.squadair.mobile.service

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.media.AudioAttributes
import android.media.RingtoneManager
import android.os.Build
import android.os.PowerManager
import androidx.core.app.NotificationCompat
import androidx.core.app.Person
import androidx.core.content.ContextCompat
import androidx.core.graphics.drawable.IconCompat
import app.squadair.mobile.MainActivity
import app.squadair.mobile.R
import uniffi.squad_core.MobileIncomingCall
import java.nio.ByteBuffer

enum class CallNotificationAction {
    NONE,
    POST_NOTIFICATION,
    CANCEL_NOTIFICATION,
}

data class CallNotificationDecision(
    val action: CallNotificationAction,
    val shouldUpdateIncomingCallState: Boolean = false,
    val clearedIncomingCall: Boolean = false,
)

class CallNotificationCoordinator {
    var currentInviteUri: String? = null
        private set
    var postedInviteUri: String? = null
        private set

    fun onIncomingCall(inviteUri: String, isUiVisible: Boolean): CallNotificationDecision {
        currentInviteUri = inviteUri
        if (isUiVisible) {
            val action = if (postedInviteUri != null) {
                postedInviteUri = null
                CallNotificationAction.CANCEL_NOTIFICATION
            } else {
                CallNotificationAction.NONE
            }
            return CallNotificationDecision(
                action = action,
                shouldUpdateIncomingCallState = true,
            )
        }
        if (postedInviteUri == inviteUri) {
            return CallNotificationDecision(
                action = CallNotificationAction.NONE,
                shouldUpdateIncomingCallState = false,
            )
        }
        postedInviteUri = inviteUri
        return CallNotificationDecision(
            action = CallNotificationAction.POST_NOTIFICATION,
            shouldUpdateIncomingCallState = true,
        )
    }

    fun onAppForegrounded(): CallNotificationAction {
        return if (postedInviteUri != null) {
            postedInviteUri = null
            CallNotificationAction.CANCEL_NOTIFICATION
        } else {
            CallNotificationAction.NONE
        }
    }

    fun onAccept(): CallNotificationAction = clearAll()

    fun onDecline(): CallNotificationAction = clearAll()

    fun onDismissed(): CallNotificationAction = clearAll()

    fun onJoinedRoom(): CallNotificationAction = clearAll()

    fun onTimeout(inviteUri: String): CallNotificationDecision {
        if (currentInviteUri != inviteUri) {
            return CallNotificationDecision(action = CallNotificationAction.NONE)
        }
        val wasPosted = postedInviteUri != null
        currentInviteUri = null
        postedInviteUri = null
        return CallNotificationDecision(
            action = if (wasPosted) CallNotificationAction.CANCEL_NOTIFICATION else CallNotificationAction.NONE,
            clearedIncomingCall = true,
        )
    }

    private fun clearAll(): CallNotificationAction {
        currentInviteUri = null
        return if (postedInviteUri != null) {
            postedInviteUri = null
            CallNotificationAction.CANCEL_NOTIFICATION
        } else {
            CallNotificationAction.NONE
        }
    }
}

object IncomingCallNotificationManager {
    const val CHANNEL_ID = "squadair_incoming_calls"
    const val NOTIFICATION_ID = 3479
    const val CALL_TIMEOUT_MS = 35_000L

    const val ACTION_SHOW_INCOMING_CALL = "app.squadair.mobile.action.SHOW_INCOMING_CALL"
    const val ACTION_ACCEPT_INCOMING_CALL = "app.squadair.mobile.action.ACCEPT_INCOMING_CALL"
    const val ACTION_DECLINE_INCOMING_CALL = "app.squadair.mobile.action.DECLINE_INCOMING_CALL"
    const val ACTION_DISMISS_INCOMING_CALL = "app.squadair.mobile.action.DISMISS_INCOMING_CALL"
    const val EXTRA_INVITE_URI = "extra_invite_uri"

    private const val WAKE_LOCK_TAG = "SquadAir::IncomingCallWakeLock"
    private const val WAKE_LOCK_TIMEOUT_MS = 10_000L

    fun ensureChannel(context: Context) {
        val nm = context.getSystemService(NotificationManager::class.java) ?: return
        val existing = nm.getNotificationChannel(CHANNEL_ID)
        if (existing != null) return

        val ringtoneUri = RingtoneManager.getDefaultUri(RingtoneManager.TYPE_RINGTONE)
        val audioAttrs = AudioAttributes.Builder()
            .setUsage(AudioAttributes.USAGE_NOTIFICATION_RINGTONE)
            .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
            .build()

        val channel = NotificationChannel(
            CHANNEL_ID,
            "Incoming Voice Calls",
            NotificationManager.IMPORTANCE_HIGH,
        ).apply {
            description = "Heads-up alerts for incoming SquadAir voice calls and invites"
            lockscreenVisibility = Notification.VISIBILITY_PUBLIC
            setSound(ringtoneUri, audioAttrs)
            enableVibration(true)
            vibrationPattern = longArrayOf(0L, 600L, 400L, 600L, 400L)
            setShowBadge(true)
        }
        nm.createNotificationChannel(channel)
    }

    fun showIncomingCall(
        context: Context,
        call: MobileIncomingCall,
        avatarRgba: ByteArray?,
    ) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            val granted = ContextCompat.checkSelfPermission(
                context,
                Manifest.permission.POST_NOTIFICATIONS,
            ) == PackageManager.PERMISSION_GRANTED
            if (!granted) return
        }

        val pm = context.getSystemService(Context.POWER_SERVICE) as? PowerManager
        pm?.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, WAKE_LOCK_TAG)?.apply {
            setReferenceCounted(false)
            try {
                acquire(WAKE_LOCK_TIMEOUT_MS)
            } catch (_: Throwable) {
            }
        }

        ensureChannel(context)
        val nm = context.getSystemService(NotificationManager::class.java) ?: return

        val callerName = call.senderNickname.ifBlank { "SquadAir Caller" }
        val verificationLabel = when {
            call.isKnownFriend && call.isPartyInvite -> "Verified Friend • Party Invite"
            call.isKnownFriend -> "Verified Friend"
            call.isPartyInvite -> "X25519 Party Invite"
            else -> "X25519 Call"
        }

        val avatarIcon = decodeAvatarIcon(avatarRgba)
        val callerBuilder = Person.Builder()
            .setName(callerName)
            .setImportant(true)
        if (avatarIcon != null) {
            callerBuilder.setIcon(avatarIcon)
        }
        val caller = callerBuilder.build()

        val fullScreenIntent = Intent(context, MainActivity::class.java).apply {
            action = ACTION_SHOW_INCOMING_CALL
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or
                Intent.FLAG_ACTIVITY_SINGLE_TOP or
                Intent.FLAG_ACTIVITY_CLEAR_TOP
        }
        val fullScreenPendingIntent = PendingIntent.getActivity(
            context,
            1001,
            fullScreenIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

        val acceptIntent = Intent(context, MainActivity::class.java).apply {
            action = ACTION_ACCEPT_INCOMING_CALL
            putExtra(EXTRA_INVITE_URI, call.inviteUri)
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or
                Intent.FLAG_ACTIVITY_SINGLE_TOP or
                Intent.FLAG_ACTIVITY_CLEAR_TOP
        }
        val acceptPendingIntent = PendingIntent.getActivity(
            context,
            1002,
            acceptIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

        val declineIntent = Intent(context, IncomingCallActionReceiver::class.java).apply {
            action = ACTION_DECLINE_INCOMING_CALL
        }
        val declinePendingIntent = PendingIntent.getBroadcast(
            context,
            1003,
            declineIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

        val dismissIntent = Intent(context, IncomingCallActionReceiver::class.java).apply {
            action = ACTION_DISMISS_INCOMING_CALL
        }
        val deletePendingIntent = PendingIntent.getBroadcast(
            context,
            1004,
            dismissIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

        val baseBuilder = NotificationCompat.Builder(context, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_squadair)
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setPriority(NotificationCompat.PRIORITY_MAX)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setOngoing(true)
            .setAutoCancel(false)
            .setTimeoutAfter(CALL_TIMEOUT_MS)
            .setFullScreenIntent(fullScreenPendingIntent, true)
            .setContentIntent(fullScreenPendingIntent)
            .setDeleteIntent(deletePendingIntent)
            .addPerson(caller)

        val callStyleNotification = NotificationCompat.Builder(context, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_squadair)
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setPriority(NotificationCompat.PRIORITY_MAX)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setOngoing(true)
            .setAutoCancel(false)
            .setTimeoutAfter(CALL_TIMEOUT_MS)
            .setFullScreenIntent(fullScreenPendingIntent, true)
            .setContentIntent(fullScreenPendingIntent)
            .setDeleteIntent(deletePendingIntent)
            .addPerson(caller)
            .setStyle(
                NotificationCompat.CallStyle.forIncomingCall(
                    caller,
                    declinePendingIntent,
                    acceptPendingIntent,
                )
                    .setIsVideo(false)
                    .setVerificationText(verificationLabel),
            )
            .build()

        try {
            nm.notify(NOTIFICATION_ID, callStyleNotification)
        } catch (_: IllegalArgumentException) {
            postFallbackNotification(
                nm = nm,
                builder = baseBuilder,
                callerName = callerName,
                subtitle = if (call.isPartyInvite) "Incoming voice room invite" else "Incoming encrypted voice call",
                declinePendingIntent = declinePendingIntent,
                acceptPendingIntent = acceptPendingIntent,
            )
        } catch (_: SecurityException) {
            postFallbackNotification(
                nm = nm,
                builder = baseBuilder,
                callerName = callerName,
                subtitle = if (call.isPartyInvite) "Incoming voice room invite" else "Incoming encrypted voice call",
                declinePendingIntent = declinePendingIntent,
                acceptPendingIntent = acceptPendingIntent,
            )
        }
    }

    fun cancel(context: Context) {
        val nm = context.getSystemService(NotificationManager::class.java) ?: return
        nm.cancel(NOTIFICATION_ID)
    }

    private fun postFallbackNotification(
        nm: NotificationManager,
        builder: NotificationCompat.Builder,
        callerName: String,
        subtitle: String,
        declinePendingIntent: PendingIntent,
        acceptPendingIntent: PendingIntent,
    ) {
        val fallback = builder
            .setContentTitle(callerName)
            .setContentText(subtitle)
            .addAction(0, "Decline", declinePendingIntent)
            .addAction(0, "Accept", acceptPendingIntent)
            .build()
        try {
            nm.notify(NOTIFICATION_ID, fallback)
        } catch (_: Throwable) {
        }
    }

    private fun decodeAvatarIcon(rgbaBytes: ByteArray?): IconCompat? {
        if (rgbaBytes == null || rgbaBytes.size < 128 * 128 * 4) return null
        return try {
            val bmp = Bitmap.createBitmap(128, 128, Bitmap.Config.ARGB_8888)
            bmp.copyPixelsFromBuffer(ByteBuffer.wrap(rgbaBytes, 0, 128 * 128 * 4))
            IconCompat.createWithBitmap(bmp)
        } catch (_: Throwable) {
            null
        }
    }
}
