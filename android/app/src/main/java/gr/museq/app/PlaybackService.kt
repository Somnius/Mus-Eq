// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
package gr.museq.app

import android.net.Uri
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.datasource.DefaultDataSource
import androidx.media3.datasource.DefaultHttpDataSource
import androidx.media3.datasource.ResolvingDataSource
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.session.MediaSession
import androidx.media3.session.MediaSessionService

/** Foreground media service: ExoPlayer plus a MediaSession (lock-screen and notification controls). */
private fun portOf(u: Uri) = if (u.port >= 0) u.port else if (u.scheme == "https") 443 else 80

class PlaybackService : MediaSessionService() {
    private var session: MediaSession? = null

    override fun onCreate() {
        super.onCreate()
        // The token is added when a stream opens (so a new login works at once) and only for the configured
        // server: the service can be bound by other apps, and a URI they queue must never receive the token.
        val http = ResolvingDataSource.Factory(
            DefaultHttpDataSource.Factory().setAllowCrossProtocolRedirects(false),
            ResolvingDataSource.Resolver { spec ->
                val base = Uri.parse(Prefs.base(this))
                val u = spec.uri
                val same = u.scheme == base.scheme && u.host != null && u.host == base.host &&
                    portOf(u) == portOf(base)
                if (same) spec.withAdditionalHeaders(mapOf("Authorization" to "Bearer " + Prefs.token(this))) else spec
            })
        val player = ExoPlayer.Builder(this)
            .setMediaSourceFactory(DefaultMediaSourceFactory(ResolvingDataSource.Factory(
                DefaultDataSource.Factory(this, http),
                ResolvingDataSource.Resolver { spec -> resolveTrack(spec) })))
            .setAudioAttributes(
                AudioAttributes.Builder().setUsage(C.USAGE_MEDIA)
                    .setContentType(C.AUDIO_CONTENT_TYPE_MUSIC).build(), true)
            .setHandleAudioBecomingNoisy(true)
            .setWakeMode(C.WAKE_MODE_NETWORK)
            .build()
        session = MediaSession.Builder(this, player).build()
    }

    /** museq://track/<fileId> becomes the pinned file if there is one, otherwise the server's audio route. */
    private fun resolveTrack(spec: androidx.media3.datasource.DataSpec): androidx.media3.datasource.DataSpec {
        val u = spec.uri
        if (u.scheme != "museq" || u.host != "track") return spec
        val id = u.lastPathSegment ?: return spec
        val target = if (Pins.isPinned(this, id)) Uri.fromFile(Pins.file(this, id))
                     else Uri.parse(Prefs.base(this) + "/v1/audio/" + id)
        return spec.withUri(target)
    }

    override fun onGetSession(info: MediaSession.ControllerInfo) = session

    override fun onDestroy() {
        session?.run { player.release(); release() }
        session = null
        super.onDestroy()
    }
}
