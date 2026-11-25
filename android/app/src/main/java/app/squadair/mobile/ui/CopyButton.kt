package app.squadair.mobile.ui

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.scale
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.dp
import app.squadair.mobile.ui.theme.SquadAirOnPrimary
import app.squadair.mobile.ui.theme.SquadAirPrimary
import app.squadair.mobile.ui.theme.SquadAirSpeakingGreen
import kotlinx.coroutines.delay

@Composable
fun AnimatedCopyButton(
    textToCopy: String,
    modifier: Modifier = Modifier,
    label: String = "Copy Code",
    copiedLabel: String = "Copied!",
    tonal: Boolean = true,
) {
    val clipboard = LocalClipboardManager.current
    var copied by remember { mutableStateOf(false) }
    var copyTrigger by remember { mutableIntStateOf(0) }

    LaunchedEffect(copyTrigger) {
        if (copyTrigger > 0) {
            delay(2000L)
            copied = false
        }
    }

    val defaultContainer = if (tonal) {
        MaterialTheme.colorScheme.secondaryContainer
    } else {
        SquadAirPrimary
    }
    val defaultContent = if (tonal) {
        MaterialTheme.colorScheme.onSecondaryContainer
    } else {
        SquadAirOnPrimary
    }
    val containerColor by animateColorAsState(
        targetValue = if (copied) SquadAirSpeakingGreen else defaultContainer,
        animationSpec = tween(250),
        label = "copyButtonContainer",
    )
    val contentColor by animateColorAsState(
        targetValue = if (copied) SquadAirOnPrimary else defaultContent,
        animationSpec = tween(250),
        label = "copyButtonContent",
    )

    val content: @Composable () -> Unit = {
        AnimatedContent(
            targetState = copied,
            transitionSpec = {
                (fadeIn(tween(180)) + scaleIn(initialScale = 0.85f))
                    .togetherWith(fadeOut(tween(150)) + scaleOut(targetScale = 0.85f))
            },
            label = "copyButtonContentMorph",
        ) { isCopied ->
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(
                    imageVector = if (isCopied) Icons.Default.Check else Icons.Default.ContentCopy,
                    contentDescription = null,
                    modifier = Modifier.size(16.dp),
                )
                Spacer(modifier = Modifier.width(6.dp))
                Text(if (isCopied) copiedLabel else label)
            }
        }
    }

    val onClick = {
        clipboard.setText(AnnotatedString(textToCopy))
        copied = true
        copyTrigger += 1
    }

    if (tonal) {
        FilledTonalButton(
            onClick = onClick,
            colors = ButtonDefaults.filledTonalButtonColors(
                containerColor = containerColor,
                contentColor = contentColor,
            ),
            modifier = modifier,
        ) {
            content()
        }
    } else {
        Button(
            onClick = onClick,
            colors = ButtonDefaults.buttonColors(
                containerColor = containerColor,
                contentColor = contentColor,
            ),
            modifier = modifier,
        ) {
            content()
        }
    }
}

@Composable
fun AnimatedCopyIconButton(
    textToCopy: String,
    contentDescription: String,
    modifier: Modifier = Modifier,
    defaultTint: Color = SquadAirPrimary,
) {
    val clipboard = LocalClipboardManager.current
    var copied by remember { mutableStateOf(false) }
    var copyTrigger by remember { mutableIntStateOf(0) }

    LaunchedEffect(copyTrigger) {
        if (copyTrigger > 0) {
            delay(2000L)
            copied = false
        }
    }

    val tint by animateColorAsState(
        targetValue = if (copied) SquadAirSpeakingGreen else defaultTint,
        animationSpec = tween(250),
        label = "copyIconTint",
    )
    val scale by animateFloatAsState(
        targetValue = if (copied) 1.12f else 1f,
        animationSpec = tween(180),
        label = "copyIconScale",
    )

    IconButton(
        onClick = {
            clipboard.setText(AnnotatedString(textToCopy))
            copied = true
            copyTrigger += 1
        },
        modifier = modifier.scale(scale),
    ) {
        AnimatedContent(
            targetState = copied,
            transitionSpec = {
                (fadeIn(tween(180)) + scaleIn(initialScale = 0.8f))
                    .togetherWith(fadeOut(tween(150)) + scaleOut(targetScale = 0.8f))
            },
            label = "copyIconMorph",
        ) { isCopied ->
            Icon(
                imageVector = if (isCopied) Icons.Default.Check else Icons.Default.ContentCopy,
                contentDescription = if (isCopied) "Copied!" else contentDescription,
                tint = tint,
            )
        }
    }
}
