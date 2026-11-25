package app.squadair.mobile.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

class AppUpdaterTest {
    @Test
    fun parseSemVer_handlesTwoAndThreePartAndPreRelease() {
        assertEquals(SemVer(1, 4, 0, null), AppUpdater.parseSemVer("v1.4"))
        assertEquals(SemVer(1, 4, 0, null), AppUpdater.parseSemVer("1.4.0"))
        assertEquals(SemVer(1, 4, 0, null), AppUpdater.parseSemVer("V1.4.0"))
        assertEquals(SemVer(1, 4, 1, "rc1"), AppUpdater.parseSemVer("v1.4.1-rc1+build42"))
        assertNull(AppUpdater.parseSemVer(""))
        assertNull(AppUpdater.parseSemVer("v1"))
        assertNull(AppUpdater.parseSemVer("1.2.3.4"))
        assertNull(AppUpdater.parseSemVer("v1.x.0"))
        assertNull(AppUpdater.parseSemVer("v1.4.0-"))
        assertNull(AppUpdater.parseSemVer("v1.4.0-+build1"))
        assertNull(AppUpdater.parseSemVer("v1.4.0-rc..1"))
        assertNull(AppUpdater.parseSemVer("vV1.4.0"))
        assertNull(AppUpdater.parseSemVer("vv1.4.0"))
    }

    @Test
    fun isNewerVersion_comparesProperly() {
        assertTrue(AppUpdater.isNewerVersion("1.0.0", "v1.4"))
        assertTrue(AppUpdater.isNewerVersion("1.4.0-rc1", "v1.4.0"))
        assertFalse(AppUpdater.isNewerVersion("1.4.0", "v1.4"))
        assertFalse(AppUpdater.isNewerVersion("1.5.0", "v1.4.9"))
        assertFalse(AppUpdater.isNewerVersion("1.4.0", "1.4.0-rc1"))
    }

    @Test
    fun parseGithubReleaseJson_extractsApkAndChecksumsAndRejectsUnsafe() {
        val validJson = """
            {
              "tag_name": "v1.5.0",
              "draft": false,
              "prerelease": false,
              "html_url": "https://github.com/alivergg/squad-air/releases/tag/v1.5.0",
              "body": "Line 1\n\nLine 2",
              "assets": [
                {
                  "name": "SquadAir-android-arm64.apk",
                  "browser_download_url": "https://github.com/alivergg/squad-air/releases/download/v1.5.0/SquadAir-android-arm64.apk"
                },
                {
                  "name": "SHA256SUMS.txt",
                  "browser_download_url": "https://github.com/alivergg/squad-air/releases/download/v1.5.0/SHA256SUMS.txt"
                }
              ]
            }
        """.trimIndent()

        val parsed = AppUpdater.parseGithubReleaseJson(validJson)
        assertNotNull(parsed)
        assertEquals("v1.5.0", parsed?.tagName)
        assertEquals("1.5.0", parsed?.versionName)
        assertEquals(
            "https://github.com/alivergg/squad-air/releases/download/v1.5.0/SquadAir-android-arm64.apk",
            parsed?.apkUrl,
        )
        assertEquals(
            "https://github.com/alivergg/squad-air/releases/download/v1.5.0/SHA256SUMS.txt",
            parsed?.sha256SumsUrl,
        )
        assertEquals("Line 1\nLine 2", parsed?.releaseNotes)

        val draftJson = validJson.replace("\"draft\": false", "\"draft\": true")
        assertNull(AppUpdater.parseGithubReleaseJson(draftJson))

        val preJson = validJson.replace("\"prerelease\": false", "\"prerelease\": true")
        assertNull(AppUpdater.parseGithubReleaseJson(preJson))

        val untrustedApkJson = validJson.replace(
            "https://github.com/alivergg/squad-air/releases/download/v1.5.0/SquadAir-android-arm64.apk",
            "https://evil.example.com/SquadAir-android-arm64.apk",
        )
        assertNull(AppUpdater.parseGithubReleaseJson(untrustedApkJson))
    }

    @Test
    fun parseSha256ForAsset_andComputeFileSha256_matchExpectedDigest() {
        val temp = File.createTempFile("squadair-sha-test", ".bin")
        try {
            temp.writeBytes("squadair-apk-payload".toByteArray(Charsets.UTF_8))
            val actualHash = AppUpdater.computeFileSha256(temp)
            assertEquals(64, actualHash.length)

            val sumsText = """
                # Checksums
                0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  SquadAir-windows.zip
                ${actualHash.uppercase()}  *./SquadAir-android-arm64.apk
            """.trimIndent()

            assertEquals(actualHash, AppUpdater.parseSha256ForAsset(sumsText))
            assertNull(AppUpdater.parseSha256ForAsset(sumsText, "missing.apk"))
        } finally {
            temp.delete()
        }
    }
}
