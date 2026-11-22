package app.squadair.mobile.ui

import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.AttachFile
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.Image
import androidx.compose.material.icons.filled.OpenInNew
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.FileProvider
import app.squadair.mobile.core.SquadSessionRepository
import app.squadair.mobile.core.SquadUiState
import app.squadair.mobile.ui.theme.SquadAirDangerRed
import app.squadair.mobile.ui.theme.SquadAirPrimary
import app.squadair.mobile.ui.theme.SquadAirSpeakingGreen
import app.squadair.mobile.ui.theme.SquadAirSurfaceCard
import app.squadair.mobile.ui.theme.SquadAirSurfaceElevated
import app.squadair.mobile.ui.theme.SquadAirTextMuted
import app.squadair.mobile.ui.theme.SquadAirTextPrimary
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import uniffi.squad_core.MobileBlobTransferState
import uniffi.squad_core.MobileChatContent
import uniffi.squad_core.MobileChatItem
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.FileOutputStream

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ChatSheet(
    state: SquadUiState,
    repository: SquadSessionRepository,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    var textInput by remember { mutableStateOf("") }

    val imagePicker = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.PickVisualMedia(),
    ) { uri: Uri? ->
        if (uri != null) {
            scope.launch(Dispatchers.IO) {
                val compressed = compressImageForInline(context.contentResolver.openInputStream(uri))
                if (compressed != null) {
                    val name = queryDisplayName(context.contentResolver, uri) ?: "image.jpg"
                    repository.sendInlineImage(name, compressed)
                }
            }
        }
    }

    val filePicker = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.OpenDocument(),
    ) { uri: Uri? ->
        if (uri != null) {
            scope.launch(Dispatchers.IO) {
                val name = queryDisplayName(context.contentResolver, uri) ?: "shared_file.bin"
                val stagingDir = File(context.cacheDir, "outbound_files").apply { mkdirs() }
                val safeName = sanitizeStagingFilename(name)
                val dest = File(stagingDir, safeName)
                context.contentResolver.openInputStream(uri)?.use { input ->
                    FileOutputStream(dest).use { output ->
                        input.copyTo(output)
                    }
                }
                if (dest.exists() && dest.length() > 0) {
                    repository.offerFile(dest.absolutePath)
                }
            }
        }
    }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        containerColor = SquadAirSurfaceCard,
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .fillMaxHeight(0.85f)
                .padding(horizontal = 16.dp, vertical = 8.dp),
        ) {
            Text(
                text = "Room Chat & Mini-Swarm Files",
                color = SquadAirTextPrimary,
                fontSize = 18.sp,
                fontWeight = FontWeight.SemiBold,
            )
            Spacer(modifier = Modifier.height(10.dp))

            LazyColumn(
                modifier = Modifier
                    .weight(1f)
                    .fillMaxWidth(),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                items(
                    items = state.chatMessages,
                    key = { "${it.senderUuidHex}_${it.msgId}" },
                ) { item ->
                    ChatMessageCard(
                        item = item,
                        onDownload = { blobId -> repository.downloadBlob(blobId) },
                        onOpenFile = { path ->
                            val file = File(path)
                            if (file.exists()) {
                                val contentUri = FileProvider.getUriForFile(
                                    context,
                                    "${context.packageName}.fileprovider",
                                    file,
                                )
                                val intent = Intent(Intent.ACTION_VIEW).apply {
                                    setDataAndType(contentUri, "*/*")
                                    addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                                }
                                context.startActivity(Intent.createChooser(intent, "Open ${file.name}"))
                            }
                        },
                    )
                }
            }

            Spacer(modifier = Modifier.height(8.dp))

            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(bottom = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                IconButton(
                    onClick = {
                        imagePicker.launch(
                            PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly),
                        )
                    },
                ) {
                    Icon(Icons.Default.Image, contentDescription = "Send image", tint = SquadAirPrimary)
                }
                IconButton(onClick = { filePicker.launch(arrayOf("*/*")) }) {
                    Icon(Icons.Default.AttachFile, contentDescription = "Send file", tint = SquadAirPrimary)
                }
                OutlinedTextField(
                    value = textInput,
                    onValueChange = { textInput = it },
                    placeholder = { Text("Message room...") },
                    singleLine = true,
                    modifier = Modifier.weight(1f),
                )
                IconButton(
                    onClick = {
                        repository.sendChatText(textInput)
                        textInput = ""
                    },
                    enabled = textInput.isNotBlank(),
                ) {
                    Icon(Icons.AutoMirrored.Filled.Send, contentDescription = "Send", tint = SquadAirPrimary)
                }
            }
        }
    }
}

@Composable
private fun ChatMessageCard(
    item: MobileChatItem,
    onDownload: (String) -> Unit,
    onOpenFile: (String) -> Unit,
) {
    Card(
        colors = CardDefaults.cardColors(containerColor = SquadAirSurfaceElevated),
        shape = RoundedCornerShape(12.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(modifier = Modifier.padding(12.dp)) {
            Text(
                text = if (item.isSelf) "${item.senderNickname} (You)" else item.senderNickname,
                color = if (item.isSelf) SquadAirPrimary else SquadAirSpeakingGreen,
                fontSize = 12.sp,
                fontWeight = FontWeight.SemiBold,
            )
            Spacer(modifier = Modifier.height(4.dp))

            when (val content = item.content) {
                is MobileChatContent.Text -> {
                    Text(
                        text = content.text,
                        color = SquadAirTextPrimary,
                        fontSize = 14.sp,
                    )
                }

                is MobileChatContent.InlineImage -> {
                    val bitmap = remember(content.bytes) {
                        content.bytes?.let { raw ->
                            decodeBoundedBitmap(raw, maxDim = 2048)?.asImageBitmap()
                        }
                    }
                    if (bitmap != null) {
                        Image(
                            bitmap = bitmap,
                            contentDescription = content.filename,
                            contentScale = ContentScale.FillWidth,
                            modifier = Modifier
                                .fillMaxWidth()
                                .clip(RoundedCornerShape(8.dp)),
                        )
                    } else {
                        BlobTransferControls(
                            filename = content.filename,
                            totalSize = content.totalSize,
                            state = content.state,
                            onDownload = { onDownload(content.blobIdHex) },
                            onOpenFile = onOpenFile,
                        )
                    }
                }

                is MobileChatContent.File -> {
                    BlobTransferControls(
                        filename = content.filename,
                        totalSize = content.totalSize,
                        state = content.state,
                        onDownload = { onDownload(content.blobIdHex) },
                        onOpenFile = onOpenFile,
                    )
                }
            }
        }
    }
}

@Composable
private fun BlobTransferControls(
    filename: String,
    totalSize: ULong,
    state: MobileBlobTransferState,
    onDownload: () -> Unit,
    onOpenFile: (String) -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text = filename,
                    color = SquadAirTextPrimary,
                    fontSize = 13.sp,
                    fontWeight = FontWeight.Medium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    text = formatBytes(totalSize.toLong()),
                    color = SquadAirTextMuted,
                    fontSize = 11.sp,
                )
            }
            when (state) {
                is MobileBlobTransferState.Available -> {
                    FilledTonalButton(onClick = onDownload) {
                        Icon(Icons.Default.Download, contentDescription = null, modifier = Modifier.size(16.dp))
                        Spacer(modifier = Modifier.width(4.dp))
                        Text("Download", fontSize = 12.sp)
                    }
                }
                is MobileBlobTransferState.Completed -> {
                    state.savedPath?.let { path ->
                        FilledTonalButton(onClick = { onOpenFile(path) }) {
                            Icon(Icons.Default.OpenInNew, contentDescription = null, modifier = Modifier.size(16.dp))
                            Spacer(modifier = Modifier.width(4.dp))
                            Text("Open / Share", fontSize = 12.sp)
                        }
                    }
                }
                else -> {}
            }
        }

        when (state) {
            is MobileBlobTransferState.Downloading -> {
                val total = state.totalChunks.toFloat().coerceAtLeast(1f)
                val progress = (state.receivedChunks.toFloat() / total).coerceIn(0f, 1f)
                LinearProgressIndicator(
                    progress = { progress },
                    color = SquadAirPrimary,
                    modifier = Modifier.fillMaxWidth(),
                )
                Text(
                    text = "Downloading ${state.receivedChunks}/${state.totalChunks} chunks...",
                    color = SquadAirTextMuted,
                    fontSize = 11.sp,
                )
            }
            is MobileBlobTransferState.Failed -> {
                Text(text = state.reason, color = SquadAirDangerRed, fontSize = 11.sp)
            }
            else -> {}
        }
    }
}

private fun queryDisplayName(resolver: android.content.ContentResolver, uri: Uri): String? {
    resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
        if (cursor.moveToFirst()) {
            val idx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
            if (idx >= 0) return cursor.getString(idx)
        }
    }
    return null
}

private fun sanitizeStagingFilename(raw: String): String {
    val cleaned = raw
        .substringAfterLast('/')
        .substringAfterLast('\\')
        .replace(Regex("[^A-Za-z0-9._-]"), "_")
        .trimStart('.')
        .take(128)
    return if (cleaned.isEmpty() || cleaned == "." || cleaned == "..") {
        "shared_file.bin"
    } else {
        cleaned
    }
}

private fun decodeBoundedBitmap(raw: ByteArray, maxDim: Int): Bitmap? {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(raw, 0, raw.size, bounds)
    val w = bounds.outWidth
    val h = bounds.outHeight
    if (w <= 0 || h <= 0 || w > 4096 || h > 4096) {
        return null
    }
    var sampleSize = 1
    while ((w / sampleSize) > maxDim || (h / sampleSize) > maxDim) {
        sampleSize *= 2
    }
    val opts = BitmapFactory.Options().apply {
        inSampleSize = sampleSize
        inPreferredConfig = Bitmap.Config.ARGB_8888
    }
    return BitmapFactory.decodeByteArray(raw, 0, raw.size, opts)
}

private fun compressImageForInline(stream: java.io.InputStream?): ByteArray? {
    stream ?: return null
    val raw = stream.use { it.readBytes() }
    if (raw.isNotEmpty() && raw.size < 190 * 1024) {
        return raw
    }
    val decoded = decodeBoundedBitmap(raw, maxDim = 960) ?: return null
    val maxSide = 960
    val ratio = minOf(maxSide.toFloat() / decoded.width, maxSide.toFloat() / decoded.height, 1f)
    val scaled = if (ratio < 1f) {
        Bitmap.createScaledBitmap(
            decoded,
            (decoded.width * ratio).toInt().coerceAtLeast(1),
            (decoded.height * ratio).toInt().coerceAtLeast(1),
            true,
        )
    } else {
        decoded
    }
    val out = ByteArrayOutputStream()
    for (q in intArrayOf(82, 65, 48, 32)) {
        out.reset()
        scaled.compress(Bitmap.CompressFormat.JPEG, q, out)
        if (out.size() < 190 * 1024) {
            return out.toByteArray()
        }
    }
    return null
}

private fun formatBytes(bytes: Long): String {
    return when {
        bytes < 1024 -> "$bytes B"
        bytes < 1024 * 1024 -> "${bytes / 1024} KB"
        else -> String.format("%.1f MB", bytes / (1024.0 * 1024.0))
    }
}
