package app.squadair.mobile.service

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import app.squadair.mobile.core.SquadSessionRepository

class IncomingCallActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent?) {
        val repo = SquadSessionRepository.getInstance(context)
        when (intent?.action) {
            IncomingCallNotificationManager.ACTION_DECLINE_INCOMING_CALL -> {
                repo.declineIncomingCall()
                IncomingCallNotificationManager.cancel(context)
            }
            IncomingCallNotificationManager.ACTION_DISMISS_INCOMING_CALL -> {
                repo.dismissIncomingCallNotification()
            }
        }
    }
}
