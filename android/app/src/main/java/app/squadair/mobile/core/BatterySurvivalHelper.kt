package app.squadair.mobile.core

import android.content.ActivityNotFoundException
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.PowerManager
import android.provider.Settings

enum class OemFamily(
    val displayName: String,
    val guidanceSteps: List<String>,
) {
    SAMSUNG(
        displayName = "Samsung",
        guidanceSteps = listOf(
            "Open Battery optimization and allow SquadAir as Unrestricted.",
            "In Device Care -> Battery -> Background usage limits, add SquadAir to Never sleeping apps.",
        ),
    ),
    XIAOMI(
        displayName = "Xiaomi / MIUI",
        guidanceSteps = listOf(
            "Enable Autostart for SquadAir in Security / Permissions.",
            "Set Battery saver to No restrictions in App battery saver.",
        ),
    ),
    OPPO_ONEPLUS(
        displayName = "OPPO / OnePlus",
        guidanceSteps = listOf(
            "Allow Auto-launch / Background activity for SquadAir.",
            "Disable battery optimization under App Info -> Battery usage.",
        ),
    ),
    VIVO(
        displayName = "Vivo",
        guidanceSteps = listOf(
            "Allow High background power consumption for SquadAir.",
            "Enable Autostart in iManager / App permissions.",
        ),
    ),
    HUAWEI_HONOR(
        displayName = "Huawei / Honor",
        guidanceSteps = listOf(
            "Open App launch and switch SquadAir to Manage manually.",
            "Enable Auto-launch, Secondary launch, and Run in background.",
        ),
    ),
    GENERIC(
        displayName = "Android",
        guidanceSteps = listOf(
            "Allow SquadAir to run unrestricted in Battery settings so incoming calls and presence stay responsive in the background.",
        ),
    ),
}

data class OemIntentSpec(
    val packageName: String? = null,
    val className: String? = null,
    val action: String? = null,
    val dataUri: String? = null,
    val extras: Map<String, String> = emptyMap(),
    val packageLaunchOnly: Boolean = false,
)

data class ResolveOutcome(
    val found: Boolean,
    val exported: Boolean,
)

object BatterySurvivalHelper {
    private const val PREFS_NAME = "squadair_prefs"
    private const val KEY_ONBOARDING_SHOWN = "battery_onboarding_shown"
    private const val ACTION_IGNORE_BATTERY_SETTINGS = "android.settings.IGNORE_BATTERY_OPTIMIZATION_SETTINGS"
    private const val ACTION_APP_DETAILS = "android.settings.APPLICATION_DETAILS_SETTINGS"

    fun shouldShowFirstLaunchOnboarding(
        prefsShown: Boolean,
        isIgnoringBatteryOpt: Boolean,
    ): Boolean = !prefsShown && !isIgnoringBatteryOpt

    fun detectOemFamily(
        manufacturer: String = Build.MANUFACTURER.orEmpty(),
        brand: String = Build.BRAND.orEmpty(),
    ): OemFamily {
        val m = manufacturer.trim().lowercase()
        val b = brand.trim().lowercase()
        val combined = "$m $b"
        return when {
            combined.contains("samsung") -> OemFamily.SAMSUNG
            combined.contains("xiaomi") || combined.contains("redmi") || combined.contains("poco") -> OemFamily.XIAOMI
            combined.contains("oppo") || combined.contains("oneplus") || combined.contains("realme") -> OemFamily.OPPO_ONEPLUS
            combined.contains("vivo") || combined.contains("iqoo") -> OemFamily.VIVO
            combined.contains("huawei") || combined.contains("honor") -> OemFamily.HUAWEI_HONOR
            else -> OemFamily.GENERIC
        }
    }

    fun candidateSpecsFor(oem: OemFamily, appPackageName: String): List<OemIntentSpec> {
        val oemSpecific = when (oem) {
            OemFamily.SAMSUNG -> listOf(
                OemIntentSpec(
                    packageName = "com.samsung.android.lool",
                    className = "com.samsung.android.sm.battery.ui.BatteryActivity",
                ),
                OemIntentSpec(
                    packageName = "com.samsung.android.lool",
                    className = "com.samsung.android.sm.ui.battery.BatteryActivity",
                ),
                OemIntentSpec(
                    packageName = "com.samsung.android.sm",
                    className = "com.samsung.android.sm.battery.ui.BatteryActivity",
                ),
                OemIntentSpec(
                    packageName = "com.samsung.android.lool",
                    packageLaunchOnly = true,
                ),
            )
            OemFamily.XIAOMI -> listOf(
                OemIntentSpec(
                    packageName = "com.miui.securitycenter",
                    className = "com.miui.permcenter.autostart.AutoStartManagementActivity",
                ),
                OemIntentSpec(
                    packageName = "com.miui.securitycenter",
                    className = "com.miui.powercenter.PowerSettings",
                ),
                OemIntentSpec(
                    action = "miui.intent.action.APP_PERM_EDITOR",
                    extras = mapOf("extra_pkgname" to appPackageName),
                ),
            )
            OemFamily.OPPO_ONEPLUS -> listOf(
                OemIntentSpec(
                    packageName = "com.coloros.safecenter",
                    className = "com.coloros.safecenter.permission.startup.StartupAppListActivity",
                ),
                OemIntentSpec(
                    packageName = "com.oplus.safecenter",
                    className = "com.oplus.safecenter.permission.startup.StartupAppListActivity",
                ),
                OemIntentSpec(
                    packageName = "com.coloros.safecenter",
                    className = "com.coloros.safecenter.startupapp.StartupAppListActivity",
                ),
                OemIntentSpec(
                    packageName = "com.oneplus.security",
                    className = "com.oneplus.security.chainlaunch.view.ChainLaunchAppListActivity",
                ),
            )
            OemFamily.VIVO -> listOf(
                OemIntentSpec(
                    packageName = "com.iqoo.secure",
                    className = "com.iqoo.secure.ui.phoneoptimize.AddWhiteListActivity",
                ),
                OemIntentSpec(
                    packageName = "com.vivo.permissionmanager",
                    className = "com.vivo.permissionmanager.activity.BgStartUpManagerActivity",
                ),
                OemIntentSpec(
                    packageName = "com.iqoo.secure",
                    className = "com.iqoo.secure.ui.phoneoptimize.BgStartUpManagerActivity",
                ),
            )
            OemFamily.HUAWEI_HONOR -> listOf(
                OemIntentSpec(
                    packageName = "com.huawei.systemmanager",
                    className = "com.huawei.systemmanager.startupmgr.ui.StartupNormalAppListActivity",
                ),
                OemIntentSpec(
                    packageName = "com.huawei.systemmanager",
                    className = "com.huawei.systemmanager.optimize.process.ProtectActivity",
                ),
                OemIntentSpec(
                    packageName = "com.hihonor.systemmanager",
                    className = "com.hihonor.systemmanager.startupmgr.ui.StartupNormalAppListActivity",
                ),
            )
            OemFamily.GENERIC -> emptyList()
        }
        val fallbacks = listOf(
            OemIntentSpec(action = ACTION_IGNORE_BATTERY_SETTINGS),
            OemIntentSpec(
                action = ACTION_APP_DETAILS,
                dataUri = "package:$appPackageName",
            ),
        )
        return oemSpecific + fallbacks
    }

    fun selectFirstLaunchableSpec(
        specs: List<OemIntentSpec>,
        resolver: (OemIntentSpec) -> ResolveOutcome,
    ): OemIntentSpec? {
        return specs.firstOrNull { spec ->
            val outcome = resolver(spec)
            outcome.found && outcome.exported
        }
    }

    fun isIgnoringBatteryOptimizations(context: Context): Boolean {
        val pm = context.getSystemService(Context.POWER_SERVICE) as? PowerManager ?: return true
        return pm.isIgnoringBatteryOptimizations(context.packageName)
    }

    fun shouldPromptOnboarding(context: Context): Boolean {
        val prefs = context.applicationContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val shown = prefs.getBoolean(KEY_ONBOARDING_SHOWN, false)
        return shouldShowFirstLaunchOnboarding(
            prefsShown = shown,
            isIgnoringBatteryOpt = isIgnoringBatteryOptimizations(context),
        )
    }

    fun markOnboardingShown(context: Context) {
        val prefs = context.applicationContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        prefs.edit().putBoolean(KEY_ONBOARDING_SHOWN, true).apply()
    }

    fun requestIgnoreBatteryOptimizations(context: Context): Boolean {
        val directIntent = Intent(
            Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS,
            Uri.parse("package:${context.packageName}"),
        ).apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        val pm = context.packageManager
        val resolved = pm.resolveActivity(directIntent, PackageManager.MATCH_DEFAULT_ONLY)
        if (resolved?.activityInfo?.exported == true) {
            try {
                context.startActivity(directIntent)
                return true
            } catch (_: SecurityException) {
            } catch (_: ActivityNotFoundException) {
            } catch (_: Throwable) {
            }
        }
        return resolveAndLaunch(
            context,
            listOf(
                OemIntentSpec(action = ACTION_IGNORE_BATTERY_SETTINGS),
                OemIntentSpec(action = ACTION_APP_DETAILS, dataUri = "package:${context.packageName}"),
            ),
        )
    }

    fun openOemBackgroundSettings(context: Context): Boolean {
        val specs = candidateSpecsFor(detectOemFamily(), context.packageName)
        return resolveAndLaunch(context, specs)
    }

    fun resolveAndLaunch(context: Context, specs: List<OemIntentSpec>): Boolean {
        val pm = context.packageManager
        for (spec in specs) {
            val intent = buildIntent(pm, spec) ?: continue
            val resolveInfo = pm.resolveActivity(intent, PackageManager.MATCH_DEFAULT_ONLY) ?: continue
            if (!resolveInfo.activityInfo.exported) continue
            intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            try {
                context.startActivity(intent)
                return true
            } catch (_: SecurityException) {
            } catch (_: ActivityNotFoundException) {
            } catch (_: Throwable) {
            }
        }
        return false
    }

    private fun buildIntent(pm: PackageManager, spec: OemIntentSpec): Intent? {
        if (spec.packageLaunchOnly && !spec.packageName.isNullOrBlank()) {
            return pm.getLaunchIntentForPackage(spec.packageName)
        }
        val intent = Intent()
        if (!spec.action.isNullOrBlank()) {
            intent.action = spec.action
        }
        if (!spec.packageName.isNullOrBlank() && !spec.className.isNullOrBlank()) {
            intent.component = ComponentName(spec.packageName, spec.className)
        }
        if (!spec.dataUri.isNullOrBlank()) {
            intent.data = Uri.parse(spec.dataUri)
        }
        for ((k, v) in spec.extras) {
            intent.putExtra(k, v)
        }
        return intent
    }
}
