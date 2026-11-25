package app.squadair.mobile.core

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.core.content.FileProvider
import app.squadair.mobile.BuildConfig
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import javax.net.ssl.HttpsURLConnection

data class SemVer(
    val major: Int,
    val minor: Int,
    val patch: Int,
    val preRelease: String? = null,
) : Comparable<SemVer> {
    override fun compareTo(other: SemVer): Int {
        if (major != other.major) return major.compareTo(other.major)
        if (minor != other.minor) return minor.compareTo(other.minor)
        if (patch != other.patch) return patch.compareTo(other.patch)
        if (preRelease == null && other.preRelease == null) return 0
        if (preRelease == null) return 1
        if (other.preRelease == null) return -1

        val aParts = preRelease.split('.')
        val bParts = other.preRelease.split('.')
        val count = maxOf(aParts.size, bParts.size)
        for (i in 0 until count) {
            if (i >= aParts.size) return -1
            if (i >= bParts.size) return 1
            val aPart = aParts[i]
            val bPart = bParts[i]
            val aNum = aPart.toIntOrNull()
            val bNum = bPart.toIntOrNull()
            val cmp = when {
                aNum != null && bNum != null -> aNum.compareTo(bNum)
                aNum != null -> -1
                bNum != null -> 1
                else -> aPart.compareTo(bPart)
            }
            if (cmp != 0) return cmp
        }
        return 0
    }
}

data class ReleaseUpdateInfo(
    val tagName: String,
    val versionName: String,
    val apkUrl: String,
    val sha256SumsUrl: String?,
    val htmlUrl: String,
    val releaseNotes: String,
)

sealed interface UpdateState {
    data object Idle : UpdateState
    data object Checking : UpdateState
    data class UpToDate(val currentVersion: String) : UpdateState
    data class Available(val release: ReleaseUpdateInfo) : UpdateState
    data class Downloading(val release: ReleaseUpdateInfo, val progress: Float?) : UpdateState
    data class ReadyToInstall(val release: ReleaseUpdateInfo, val apkFile: File) : UpdateState
    data class NeedsInstallPermission(val release: ReleaseUpdateInfo, val apkFile: File) : UpdateState
    data class Error(val message: String, val release: ReleaseUpdateInfo? = null) : UpdateState
}

object AppUpdater {
    const val LATEST_RELEASE_API_URL = "https://api.github.com/repos/alivergg/squad-air/releases/latest"
    const val ALLOWED_RELEASE_URL_PREFIX = "https://github.com/alivergg/squad-air/"
    const val ALLOWED_DOWNLOAD_PREFIX = "https://github.com/alivergg/squad-air/releases/download/"
    const val APK_ASSET_NAME = "SquadAir-android-arm64.apk"
    const val SHA256_SUMS_ASSET_NAME = "SHA256SUMS.txt"

    private const val MAX_JSON_BYTES = 1_048_576
    private const val MAX_SHA_SUMS_BYTES = 65_536
    private const val MAX_APK_BYTES = 134_217_728L
    private const val MAX_REDIRECTS = 5

    fun parseSemVer(raw: String): SemVer? {
        val rawTrimmed = raw.trim()
        val trimmed = when {
            rawTrimmed.startsWith("v") || rawTrimmed.startsWith("V") -> rawTrimmed.substring(1)
            else -> rawTrimmed
        }
        if (trimmed.isEmpty()) return null
        val withoutBuild = trimmed.substringBefore('+')
        val parts = withoutBuild.split('-', limit = 2)
        val core = parts[0]
        val pre = if (parts.size == 2) {
            val rawPre = parts[1]
            if (rawPre.isBlank() || rawPre.split('.').any { it.isEmpty() }) {
                return null
            }
            rawPre
        } else {
            null
        }
        val nums = core.split('.')
        if (nums.size !in 2..3) return null
        val major = nums[0].toIntOrNull()?.takeIf { it >= 0 } ?: return null
        val minor = nums[1].toIntOrNull()?.takeIf { it >= 0 } ?: return null
        val patch = if (nums.size == 3) {
            nums[2].toIntOrNull()?.takeIf { it >= 0 } ?: return null
        } else {
            0
        }
        return SemVer(major = major, minor = minor, patch = patch, preRelease = pre)
    }

    fun isNewerVersion(current: String, candidate: String): Boolean {
        val cur = parseSemVer(current) ?: return false
        val cand = parseSemVer(candidate) ?: return false
        return cand > cur
    }

    fun parseGithubReleaseJson(json: String): ReleaseUpdateInfo? {
        return try {
            val root = JSONObject(json)
            if (root.optBoolean("draft", false) || root.optBoolean("prerelease", false)) {
                return null
            }
            val tagName = root.optString("tag_name", "").trim()
            val semVer = parseSemVer(tagName) ?: return null
            val htmlUrl = root.optString("html_url", "").trim()
            if (!htmlUrl.startsWith(ALLOWED_RELEASE_URL_PREFIX)) {
                return null
            }

            val assets = root.optJSONArray("assets") ?: return null
            var apkUrl: String? = null
            var shaUrl: String? = null
            for (i in 0 until assets.length()) {
                val asset = assets.optJSONObject(i) ?: continue
                val name = asset.optString("name", "").trim()
                val downloadUrl = asset.optString("browser_download_url", "").trim()
                if (!downloadUrl.startsWith(ALLOWED_DOWNLOAD_PREFIX)) continue
                if (name == APK_ASSET_NAME || downloadUrl.endsWith("/$APK_ASSET_NAME")) {
                    apkUrl = downloadUrl
                } else if (name == SHA256_SUMS_ASSET_NAME) {
                    shaUrl = downloadUrl
                }
            }
            val resolvedApkUrl = apkUrl ?: return null
            val notes = root.optString("body", "")
                .lines()
                .map { it.trim() }
                .filter { it.isNotEmpty() }
                .take(12)
                .joinToString("\n")
            ReleaseUpdateInfo(
                tagName = tagName,
                versionName = "${semVer.major}.${semVer.minor}.${semVer.patch}" +
                    (semVer.preRelease?.let { "-$it" } ?: ""),
                apkUrl = resolvedApkUrl,
                sha256SumsUrl = shaUrl,
                htmlUrl = htmlUrl,
                releaseNotes = notes,
            )
        } catch (_: Throwable) {
            null
        }
    }

    fun parseSha256ForAsset(sumsText: String, assetName: String = APK_ASSET_NAME): String? {
        for (rawLine in sumsText.lines()) {
            val line = rawLine.trim()
            if (line.isEmpty() || line.startsWith("#")) continue
            val match = Regex("^([0-9a-fA-F]{64})\\s+[*./]*(.+)$").matchEntire(line) ?: continue
            val hash = match.groupValues[1].lowercase()
            val target = match.groupValues[2].trim().removePrefix("./")
            if (target == assetName || target.endsWith("/$assetName")) {
                return hash
            }
        }
        return null
    }

    fun computeFileSha256(file: File): String {
        val digest = MessageDigest.getInstance("SHA-256")
        val buf = ByteArray(16_384)
        FileInputStream(file).use { input ->
            while (true) {
                val read = input.read(buf)
                if (read <= 0) break
                digest.update(buf, 0, read)
            }
        }
        return digest.digest().joinToString("") { "%02x".format(it) }
    }

    suspend fun checkLatestRelease(
        currentVersion: String = BuildConfig.VERSION_NAME,
    ): Result<ReleaseUpdateInfo?> = withContext(Dispatchers.IO) {
        try {
            val body = fetchHttpsText(
                url = LATEST_RELEASE_API_URL,
                maxBytes = MAX_JSON_BYTES,
                acceptHeader = "application/vnd.github+json",
                userAgent = "SquadAir-Android/$currentVersion",
                connectTimeoutMs = 10_000,
                readTimeoutMs = 15_000,
            )
            val parsed = parseGithubReleaseJson(body)
                ?: return@withContext Result.success(null)
            if (isNewerVersion(currentVersion, parsed.tagName)) {
                Result.success(parsed)
            } else {
                Result.success(null)
            }
        } catch (ce: CancellationException) {
            throw ce
        } catch (t: Throwable) {
            Result.failure(t)
        }
    }

    suspend fun downloadAndVerifyApk(
        context: Context,
        release: ReleaseUpdateInfo,
        onProgress: (Float?) -> Unit,
    ): Result<File> = withContext(Dispatchers.IO) {
        if (!release.apkUrl.startsWith(ALLOWED_DOWNLOAD_PREFIX)) {
            return@withContext Result.failure(IllegalArgumentException("Untrusted APK download URL"))
        }
        val updatesDir = File(context.cacheDir, "updates").apply { mkdirs() }
        updatesDir.listFiles()?.forEach { old ->
            if (old.name.endsWith(".apk") || old.name.endsWith(".part")) {
                old.delete()
            }
        }

        val expectedSha = if (release.sha256SumsUrl != null) {
            if (!release.sha256SumsUrl.startsWith(ALLOWED_DOWNLOAD_PREFIX)) {
                return@withContext Result.failure(IllegalArgumentException("Untrusted checksum URL"))
            }
            val sumsText = try {
                fetchHttpsText(
                    url = release.sha256SumsUrl,
                    maxBytes = MAX_SHA_SUMS_BYTES,
                    acceptHeader = "text/plain",
                    userAgent = "SquadAir-Android/${BuildConfig.VERSION_NAME}",
                    connectTimeoutMs = 10_000,
                    readTimeoutMs = 15_000,
                )
            } catch (t: Throwable) {
                return@withContext Result.failure(
                    IOException("Failed to fetch SHA256SUMS.txt: ${t.message}", t),
                )
            }
            parseSha256ForAsset(sumsText, APK_ASSET_NAME)
                ?: return@withContext Result.failure(
                    IOException("Missing $APK_ASSET_NAME entry in SHA256SUMS.txt"),
                )
        } else {
            null
        }

        val partFile = File(updatesDir, "$APK_ASSET_NAME.part")
        val finalFile = File(updatesDir, APK_ASSET_NAME)
        var conn: HttpsURLConnection? = null
        try {
            conn = openHttpsWithRedirects(
                initialUrl = release.apkUrl,
                acceptHeader = "application/vnd.android.package-archive, application/octet-stream",
                userAgent = "SquadAir-Android/${BuildConfig.VERSION_NAME}",
                connectTimeoutMs = 15_000,
                readTimeoutMs = 60_000,
            )
            val status = conn.responseCode
            if (status !in 200..299) {
                throw IOException("HTTP $status while downloading APK")
            }
            val totalBytes = conn.contentLengthLong
            if (totalBytes > MAX_APK_BYTES) {
                throw IOException("APK exceeds maximum allowed size")
            }
            var downloaded = 0L
            val buf = ByteArray(16_384)
            conn.inputStream.use { input ->
                FileOutputStream(partFile).use { output ->
                    while (true) {
                        currentCoroutineContext().ensureActive()
                        val n = input.read(buf)
                        if (n <= 0) break
                        downloaded += n
                        if (downloaded > MAX_APK_BYTES) {
                            throw IOException("APK download exceeded maximum allowed size")
                        }
                        output.write(buf, 0, n)
                        if (totalBytes > 0L) {
                            onProgress((downloaded.toFloat() / totalBytes.toFloat()).coerceIn(0f, 1f))
                        } else {
                            onProgress(null)
                        }
                    }
                }
            }
            if (expectedSha != null) {
                val actualSha = computeFileSha256(partFile)
                if (actualSha != expectedSha) {
                    throw IOException("SHA-256 checksum verification failed")
                }
            }
            if (finalFile.exists()) {
                finalFile.delete()
            }
            if (!partFile.renameTo(finalFile)) {
                partFile.copyTo(finalFile, overwrite = true)
                partFile.delete()
            }
            Result.success(finalFile)
        } catch (ce: CancellationException) {
            partFile.delete()
            throw ce
        } catch (t: Throwable) {
            partFile.delete()
            Result.failure(t)
        } finally {
            conn?.disconnect()
        }
    }

    fun canInstallPackages(context: Context): Boolean {
        return context.packageManager.canRequestPackageInstalls()
    }

    fun createUnknownSourcesSettingsIntent(context: Context): Intent {
        return Intent(
            Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
            Uri.parse("package:${context.packageName}"),
        )
    }

    fun launchApkInstaller(context: Context, apkFile: File) {
        val uri = FileProvider.getUriForFile(
            context,
            "${context.packageName}.fileprovider",
            apkFile,
        )
        val intent = Intent(Intent.ACTION_VIEW).apply {
            setDataAndType(uri, "application/vnd.android.package-archive")
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        context.startActivity(intent)
    }

    private fun fetchHttpsText(
        url: String,
        maxBytes: Int,
        acceptHeader: String,
        userAgent: String,
        connectTimeoutMs: Int,
        readTimeoutMs: Int,
    ): String {
        val conn = openHttpsWithRedirects(
            initialUrl = url,
            acceptHeader = acceptHeader,
            userAgent = userAgent,
            connectTimeoutMs = connectTimeoutMs,
            readTimeoutMs = readTimeoutMs,
        )
        try {
            val code = conn.responseCode
            if (code !in 200..299) {
                throw IOException("HTTP $code")
            }
            val bytes = conn.inputStream.use { input ->
                val out = java.io.ByteArrayOutputStream()
                val buf = ByteArray(4096)
                var total = 0
                while (true) {
                    val n = input.read(buf)
                    if (n <= 0) break
                    total += n
                    if (total > maxBytes) {
                        throw IOException("Response exceeded $maxBytes bytes")
                    }
                    out.write(buf, 0, n)
                }
                out.toByteArray()
            }
            return bytes.toString(Charsets.UTF_8)
        } finally {
            conn.disconnect()
        }
    }

    private fun openHttpsWithRedirects(
        initialUrl: String,
        acceptHeader: String,
        userAgent: String,
        connectTimeoutMs: Int,
        readTimeoutMs: Int,
    ): HttpsURLConnection {
        var current = initialUrl
        for (hop in 0..MAX_REDIRECTS) {
            if (!current.startsWith("https://")) {
                throw SecurityException("Refusing non-HTTPS URL: $current")
            }
            val conn = (URL(current).openConnection() as? HttpsURLConnection)
                ?: throw IOException("Expected HTTPS connection")
            conn.instanceFollowRedirects = false
            conn.requestMethod = "GET"
            conn.connectTimeout = connectTimeoutMs
            conn.readTimeout = readTimeoutMs
            conn.setRequestProperty("Accept", acceptHeader)
            conn.setRequestProperty("User-Agent", userAgent)
            val code = conn.responseCode
            if (code in setOf(
                    HttpURLConnection.HTTP_MOVED_PERM,
                    HttpURLConnection.HTTP_MOVED_TEMP,
                    HttpURLConnection.HTTP_SEE_OTHER,
                    307,
                    308,
                )
            ) {
                val location = conn.getHeaderField("Location")?.trim()
                conn.disconnect()
                if (location.isNullOrEmpty()) {
                    throw IOException("Redirect without Location header")
                }
                current = URL(URL(current), location).toString()
                continue
            }
            return conn
        }
        throw IOException("Too many redirects")
    }
}
