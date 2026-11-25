package app.squadair.mobile.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class BatterySurvivalHelperTest {
    @Test
    fun shouldShowFirstLaunchOnboarding_onlyWhenNotShownAndNotIgnoring() {
        assertTrue(BatterySurvivalHelper.shouldShowFirstLaunchOnboarding(prefsShown = false, isIgnoringBatteryOpt = false))
        assertFalse(BatterySurvivalHelper.shouldShowFirstLaunchOnboarding(prefsShown = true, isIgnoringBatteryOpt = false))
        assertFalse(BatterySurvivalHelper.shouldShowFirstLaunchOnboarding(prefsShown = false, isIgnoringBatteryOpt = true))
        assertFalse(BatterySurvivalHelper.shouldShowFirstLaunchOnboarding(prefsShown = true, isIgnoringBatteryOpt = true))
    }

    @Test
    fun detectOemFamily_mapsMajorManufacturersAndBrands() {
        assertEquals(OemFamily.SAMSUNG, BatterySurvivalHelper.detectOemFamily("samsung", "samsung"))
        assertEquals(OemFamily.XIAOMI, BatterySurvivalHelper.detectOemFamily("Xiaomi", "Redmi"))
        assertEquals(OemFamily.XIAOMI, BatterySurvivalHelper.detectOemFamily("Xiaomi", "POCO"))
        assertEquals(OemFamily.OPPO_ONEPLUS, BatterySurvivalHelper.detectOemFamily("OPPO", "realme"))
        assertEquals(OemFamily.OPPO_ONEPLUS, BatterySurvivalHelper.detectOemFamily("OnePlus", "OnePlus"))
        assertEquals(OemFamily.VIVO, BatterySurvivalHelper.detectOemFamily("vivo", "iQOO"))
        assertEquals(OemFamily.HUAWEI_HONOR, BatterySurvivalHelper.detectOemFamily("HUAWEI", "HONOR"))
        assertEquals(OemFamily.GENERIC, BatterySurvivalHelper.detectOemFamily("Google", "Pixel"))
    }

    @Test
    fun candidateSpecsFor_includesOemEntriesAndSystemFallbacks() {
        val pkg = "app.squadair.mobile"
        for (oem in OemFamily.entries) {
            val specs = BatterySurvivalHelper.candidateSpecsFor(oem, pkg)
            assertTrue(specs.size >= 2)
            val tail = specs.takeLast(2)
            assertEquals("android.settings.IGNORE_BATTERY_OPTIMIZATION_SETTINGS", tail[0].action)
            assertEquals("android.settings.APPLICATION_DETAILS_SETTINGS", tail[1].action)
            assertEquals("package:$pkg", tail[1].dataUri)
        }
        val samsungSpecs = BatterySurvivalHelper.candidateSpecsFor(OemFamily.SAMSUNG, pkg)
        assertTrue(samsungSpecs.any { it.packageName == "com.samsung.android.lool" })
        val xiaomiSpecs = BatterySurvivalHelper.candidateSpecsFor(OemFamily.XIAOMI, pkg)
        assertTrue(xiaomiSpecs.any { it.packageName == "com.miui.securitycenter" })
    }

    @Test
    fun selectFirstLaunchableSpec_skipsUnresolvedAndNonExportedActivities() {
        val specs = BatterySurvivalHelper.candidateSpecsFor(OemFamily.SAMSUNG, "app.squadair.mobile")
        val selected = BatterySurvivalHelper.selectFirstLaunchableSpec(specs) { spec ->
            when {
                spec.className == "com.samsung.android.sm.battery.ui.BatteryActivity" ->
                    ResolveOutcome(found = true, exported = false)
                spec.className == "com.samsung.android.sm.ui.battery.BatteryActivity" ->
                    ResolveOutcome(found = true, exported = true)
                else -> ResolveOutcome(found = false, exported = false)
            }
        }
        assertEquals("com.samsung.android.sm.ui.battery.BatteryActivity", selected?.className)
    }

    @Test
    fun selectFirstLaunchableSpec_fallsBackToSystemSettingsWhenOemUnavailable() {
        val specs = BatterySurvivalHelper.candidateSpecsFor(OemFamily.XIAOMI, "app.squadair.mobile")
        val selected = BatterySurvivalHelper.selectFirstLaunchableSpec(specs) { spec ->
            if (spec.action == "android.settings.APPLICATION_DETAILS_SETTINGS") {
                ResolveOutcome(found = true, exported = true)
            } else {
                ResolveOutcome(found = false, exported = false)
            }
        }
        assertEquals("android.settings.APPLICATION_DETAILS_SETTINGS", selected?.action)
    }
}
