package gr.museq.app

import android.Manifest
import android.app.Activity
import android.content.ComponentName
import android.net.Uri
import android.os.Bundle
import android.text.InputType
import android.widget.*
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import com.google.common.util.concurrent.ListenableFuture
import com.google.common.util.concurrent.MoreExecutors
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder
import kotlin.concurrent.thread

class Track(val id: String, val title: String, val artist: String, val fileId: String)

class MainActivity : Activity() {
    private var controllerFuture: ListenableFuture<MediaController>? = null
    private var tracks = listOf<Track>()
    private lateinit var status: TextView
    private lateinit var list: ListView
    private lateinit var battery: Button

    private fun unrestricted() =
        (getSystemService(POWER_SERVICE) as android.os.PowerManager).isIgnoringBatteryOptimizations(packageName)

    override fun onResume() {
        super.onResume()
        battery.text = if (unrestricted()) "Background playback: allowed" else "Allow background playback (battery)"
    }

    // Android's battery optimisation can stop a playing app after a while with the screen off.
    // OnePlus adds its own limits too; those are in Settings > Apps > Mus-Eq > Battery.
    private fun allowBackground() {
        val i = if (!unrestricted())
            android.content.Intent(android.provider.Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:$packageName"))
        else
            android.content.Intent(android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:$packageName"))
        startActivity(i)
    }

    override fun onCreate(b: Bundle?) {
        super.onCreate(b)
        if (b == null && android.os.Build.VERSION.SDK_INT >= 33)
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        val root = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL; setPadding(24, 48, 24, 24) }
        val url = EditText(this).apply {
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI
            hint = "http://10.0.0.3:8096"; setText(Prefs.base(context)) }
        val tok = EditText(this).apply {
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
            hint = "owner token"; setText(Prefs.token(context)) }
        val q = EditText(this).apply { hint = "search" }
        val go = Button(this).apply { text = "Connect and search" }
        status = TextView(this)
        list = ListView(this)
        battery = Button(this)
        battery.setOnClickListener { allowBackground() }
        listOf(url, tok, q, go, battery, status, list).forEach { root.addView(it) }
        setContentView(root)
        go.setOnClickListener {
            val base = url.text.toString().trim()
            if (!(base.startsWith("http://") || base.startsWith("https://"))) { status.text = "address must start with http:// or https://"; return@setOnClickListener }
            Prefs.save(this, base, tok.text.toString())
            search(q.text.toString())
        }
        list.setOnItemClickListener { _, _, i, _ -> tracks.getOrNull(i)?.let { play(it) } }
        list.setOnItemLongClickListener { _, _, i, _ -> tracks.getOrNull(i)?.let { togglePin(it) }; true }
    }

    override fun onDestroy() {
        controllerFuture?.let { MediaController.releaseFuture(it) }
        controllerFuture = null
        super.onDestroy()
    }

    private fun ui(f: () -> Unit) = runOnUiThread { if (!isDestroyed) f() }

    private fun search(q: String) {
        status.text = "searching…"
        val app = applicationContext
        thread {
            try {
                val u = URL(Prefs.base(app) + "/v1/tracks?limit=200&q=" + URLEncoder.encode(q, "UTF-8"))
                val c = u.openConnection() as HttpURLConnection
                c.instanceFollowRedirects = false
                c.setRequestProperty("Authorization", "Bearer " + Prefs.token(app))
                c.connectTimeout = 8000; c.readTimeout = 15000
                try {
                    if (c.responseCode != 200) throw RuntimeException("HTTP " + c.responseCode)
                    val j = JSONObject(c.inputStream.bufferedReader().readText())
                    val a = j.getJSONArray("items")
                    val found = (0 until a.length()).map {
                        val o = a.getJSONObject(it)
                        Track(o.getString("id"), o.getString("title"),
                            if (o.isNull("artist")) "" else o.getString("artist"), o.getString("preferred_file_id"))
                    }
                    ui { status.text = "${found.size} of ${j.getInt("total")} tracks" +
                        (if (Prefs.base(app).startsWith("http://")) " (plain http: the token is not encrypted)" else ""); show(found) }
                } finally { c.disconnect() }
            } catch (e: Exception) {
                ui { status.text = "offline or failed (" + (e.message ?: e.javaClass.simpleName) + "), showing pinned"; show(Pins.all(this)) }
            }
        }
    }

    private fun label(t: Track) = (if (Pins.isPinned(this, t.fileId)) "[offline] " else "") +
        (if (t.artist.isEmpty()) "" else t.artist + " – ") + t.title

    // always called on the UI thread, so the list the user taps and the list the code indexes are the same one
    private fun show(l: List<Track>) {
        tracks = l
        list.adapter = ArrayAdapter(this, android.R.layout.simple_list_item_1, l.map { label(it) })
    }

    private fun togglePin(t: Track) {
        if (Pins.isBusy(t.fileId)) { status.text = "already downloading"; return }
        if (Pins.isPinned(this, t.fileId)) { Pins.unpin(this, t); status.text = "unpinned"; show(tracks); return }
        status.text = "pinning…"
        val app = applicationContext
        thread {
            var last = 0L
            val err = Pins.pin(app, t) { d, tot ->
                if (System.currentTimeMillis() - last > 500) { last = System.currentTimeMillis()
                    ui { status.text = if (tot >= 0) "pinning ${d / 1048576} of ${tot / 1048576} MB" else "pinning ${d / 1048576} MB" } } }
            ui { status.text = if (err == null) "pinned" else "pin failed: $err"; show(tracks) }
        }
    }

    private fun uriOf(t: Track): Uri =
        if (Pins.isPinned(this, t.fileId)) Uri.fromFile(Pins.file(this, t.fileId))
        else Uri.parse(Prefs.base(this) + "/v1/audio/" + t.fileId)

    private fun play(tapped: Track) {
        // the queue is the whole visible list; a track that is neither pinned nor reachable stops playback with an error shown in the status line
        val start = tracks.indexOf(tapped)
        val items = tracks.map {
            MediaItem.Builder().setMediaId(it.id).setUri(uriOf(it))
                .setMediaMetadata(MediaMetadata.Builder().setTitle(it.title).setArtist(it.artist).build()).build()
        }
        val f = controllerFuture ?: MediaController.Builder(this,
            SessionToken(this, ComponentName(this, PlaybackService::class.java))).buildAsync()
            .also { fut -> controllerFuture = fut
                fut.addListener({ runCatching { fut.get() }.getOrNull()?.addListener(object : Player.Listener {
                    override fun onPlayerError(e: PlaybackException) { ui { status.text = "playback error: " + e.errorCodeName } }
                }) }, MoreExecutors.directExecutor()) }
        f.addListener({
            runCatching { f.get() }.getOrNull()?.run { setMediaItems(items, start, 0); prepare(); play() }
        }, MoreExecutors.directExecutor())
    }
}
