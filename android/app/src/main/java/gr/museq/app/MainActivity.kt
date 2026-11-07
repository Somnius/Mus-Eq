// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
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
import kotlin.concurrent.thread

/** One line of the list: what it says, what a tap does, what a long press does. */
class Row(val label: String, val onTap: () -> Unit, val onLong: (() -> Unit)? = null)

class MainActivity : Activity() {
    private var controllerFuture: ListenableFuture<MediaController>? = null
    private var rows = listOf<Row>()
    private var tracks = listOf<Track>() // the tracks behind the rows when the list is a track list
    private var view = "library"
    private var gen = 0 // bumped whenever the visible view changes; a slow job from an older view is dropped
    private lateinit var status: TextView
    private lateinit var list: ListView
    private lateinit var query: EditText
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
        query = EditText(this).apply { hint = "search (library and stations)" }
        val go = Button(this).apply { text = "Connect and search" }
        status = TextView(this)
        list = ListView(this)
        battery = Button(this)
        battery.setOnClickListener { allowBackground() }
        val tabs = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
        listOf("library" to "Library", "lists" to "Lists", "radio" to "Radio", "stations" to "Stations", "pinned" to "Pinned", "tools" to "Tools")
            .forEach { (v, name) -> tabs.addView(Button(this).apply {
                text = name; isAllCaps = false; setPadding(4, 0, 4, 0)
                layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
                setOnClickListener { open(v) } }) }
        listOf(url, tok, query, go, tabs, battery, status, list).forEach { root.addView(it) }
        setContentView(root)
        go.setOnClickListener {
            val base = url.text.toString().trim()
            if (!(base.startsWith("http://") || base.startsWith("https://"))) { status.text = "address must start with http:// or https://"; return@setOnClickListener }
            Prefs.save(this, base, tok.text.toString())
            open("library")
        }
        list.setOnItemClickListener { _, _, i, _ -> rows.getOrNull(i)?.onTap?.invoke() }
        list.setOnItemLongClickListener { _, _, i, _ -> rows.getOrNull(i)?.onLong?.invoke(); true }
    }

    override fun onDestroy() {
        controllerFuture?.let { MediaController.releaseFuture(it) }
        controllerFuture = null
        super.onDestroy()
    }

    private fun ui(f: () -> Unit) = runOnUiThread { if (!isDestroyed) f() }

    private fun showRows(l: List<Row>) {
        rows = l
        list.adapter = ArrayAdapter(this, android.R.layout.simple_list_item_1, l.map { it.label })
    }

    /** Runs the work off the UI thread; on failure the status line says why. */
    private fun background(what: String, work: () -> Unit) {
        status.text = what
        thread {
            try { work() } catch (e: Exception) {
                ui { status.text = "failed: " + (e.message ?: e.javaClass.simpleName) }
            }
        }
    }

    /** True if the screen still shows the view a background job started in. */
    private fun still(v: String) = view == v

    private fun open(v: String) {
        view = v; gen++
        when (v) {
            "library" -> library(query.text.toString())
            "lists" -> playlists()
            "radio" -> radios()
            "stations" -> stations(query.text.toString())
            "pinned" -> pinned()
            "tools" -> tools()
        }
    }

    // ---- tracks ---------------------------------------------------------------------------

    private fun label(t: Track) = (if (Pins.isPinned(this, t.fileId)) "[offline] " else "") +
        (if (t.artist.isEmpty()) "" else t.artist + " – ") + t.title

    // always called on the UI thread, so the list the user taps and the list the code indexes are the same one
    // the header rows (for example "back to lists") are remembered, so redrawing after a pin keeps them
    private var header = listOf<Row>()
    private fun showTracks(l: List<Track>, h: List<Row> = header) {
        tracks = l; header = h
        showRows(h + l.map { t -> Row(label(t), { play(t) }, { togglePin(t) }) })
    }

    private fun library(q: String) {
        val app = applicationContext
        val g = gen
        background("searching…") {
            val j = Api.obj(app, "/v1/tracks?limit=200&q=" + Api.enc(q))
            val a = j.getJSONArray("items")
            val found = (0 until a.length()).map { Parse.track(a.getJSONObject(it)) }
            ui { if (g != gen) return@ui; status.text = "${found.size} of ${j.getInt("total")} tracks" + plainHttpNote(); showTracks(found, emptyList()) }
        }
    }

    private fun plainHttpNote() =
        if (Prefs.base(this).startsWith("http://")) " (plain http: the token is not encrypted)" else ""

    private fun playlists() {
        val app = applicationContext
        val g = gen
        background("loading lists…") {
            val a = Api.arr(app, "/v1/playlists")
            val items = (0 until a.length()).map { a.getJSONObject(it) }
            ui {
                if (g != gen) return@ui
                status.text = "${items.size} lists; tap to open, long-press to pin all its tracks"
                showRows(items.map { p ->
                    val n = p.optJSONArray("track_ids")?.length() ?: 0
                    Row("${p.getString("name")} ($n)", { openPlaylist(p.getString("id"), p.getString("name")) },
                        { pinPlaylist(p.getString("id"), p.getString("name")) })
                })
            }
        }
    }

    /** The playlist's tracks in order, and how many of its ids the server no longer has. */
    private fun playlistTracks(app: android.content.Context, id: String): Pair<List<Entry>, Int> {
        val ids = Api.obj(app, "/v1/playlists/$id").getJSONArray("track_ids")
        val order = (0 until ids.length()).map { ids.getString(it) }
        val (entries, _) = Api.manifest(app, order)
        val byId = entries.associateBy { it.track.id }
        val found = order.mapNotNull { byId[it] }
        return found to (order.size - found.size)
    }

    private fun openPlaylist(id: String, name: String) {
        val app = applicationContext
        view = "playlist:$id"; gen++
        val g = gen
        background("opening $name…") {
            val (e, dropped) = playlistTracks(app, id)
            ui { if (g != gen) return@ui; status.text = "$name: ${e.size} tracks" + (if (dropped > 0) ", $dropped no longer on the server" else "")
                showTracks(e.map { it.track }, listOf(Row("‹ back to lists", { playlists() }))) }
        }
    }

    private fun pinPlaylist(id: String, name: String) {
        val app = applicationContext
        background("pinning $name…") {
            val (entries, dropped) = playlistTracks(app, id)
            var downloaded = 0; var present = 0; var busy = 0; var failed = 0
            entries.forEachIndexed { i, e ->
                // a file already here with the right size is kept as it is; "Check pinned copies" does the full hash
                if (Pins.isPinned(app, e.track.fileId) && Pins.file(app, e.track.fileId).length() == e.bytes) present++
                else {
                    ui { status.text = "$name: pinning ${i + 1} of ${entries.size}" }
                    when (Pins.pin(app, e) { _, _ -> }) {
                        null -> downloaded++
                        "already downloading" -> busy++
                        else -> failed++
                    }
                }
            }
            ui { status.text = "$name: $downloaded downloaded and checked, $present already there (size only)" +
                (if (busy > 0) ", $busy still downloading from an earlier request" else "") +
                (if (failed > 0) ", $failed failed (long-press the list again to retry)" else "") +
                (if (dropped > 0) ", $dropped no longer on the server" else "") }
        }
    }

    private fun togglePin(t: Track) {
        if (Pins.isBusy(t.fileId)) { status.text = "already downloading"; return }
        if (Pins.isPinned(this, t.fileId)) {
            Pins.unpin(this, t); status.text = "unpinned"
            if (view == "pinned") pinned() else showTracks(tracks)
            return
        }
        val app = applicationContext
        val v = view
        val gv = gen
        background("pinning…") {
            val e = Api.manifest(app, listOf(t.id)).first.firstOrNull() ?: throw RuntimeException("track not on the server")
            var last = 0L
            val err = Pins.pin(app, e) { d, tot ->
                if (System.currentTimeMillis() - last > 500) { last = System.currentTimeMillis()
                    ui { status.text = "pinning " + Parse.sizeLabel(d, tot) } } }
            ui {
                status.text = if (err == null) "pinned" else "pin failed: $err"
                // use the server's current track/file for this row, which a rescan or merge may have changed
                if (err == null) tracks = tracks.map { if (it.id == e.track.id) e.track else it }
                if (still(v) && gv == gen) showTracks(tracks)
            }
        }
    }

    private fun pinned() {
        view = "pinned"; gen++
        val all = Pins.all(this)
        status.text = "${all.size} pinned; long-press to unpin"
        showTracks(all, listOf(Row("Check pinned copies against the server", { verifyPins() })))
    }

    /** Compares every pinned file with the size and SHA-256 the server names; a differing copy is downloaded again. */
    private fun verifyPins() {
        val app = applicationContext
        val g = gen
        background("checking pinned copies…") {
            val mine = Pins.all(app)
            val (entries, missing) = Api.manifest(app, mine.map { it.id })
            var ok = 0; var fixed = 0; var failed = 0
            entries.forEachIndexed { i, e ->
                if (Pins.matches(app, e)) ok++ else {
                    ui { status.text = "re-downloading ${i + 1} of ${entries.size}" }
                    // the old copy stays until the new one has passed the size and hash check
                    if (Pins.pin(app, e) { _, _ -> } == null) {
                        fixed++
                        // the server now plays a different file for this track: drop the pin of the old one
                        mine.firstOrNull { it.id == e.track.id && it.fileId != e.track.fileId }?.let { Pins.unpin(app, it) }
                    } else failed++
                }
            }
            ui {
                if (g == gen) pinned()
                status.text = "$ok identical, $fixed re-downloaded, $failed failed, ${missing.size} no longer on the server" +
                    (if (fixed > 0) " (a track that is playing keeps its old copy until it starts again)" else "")
            }
        }
    }

    // ---- programs the server can download (ffmpeg, yt-dlp) ---------------------------------------

    private fun tools() {
        val app = applicationContext
        gen++ // a new call replaces any polling chain that is still running
        val g = gen
        background("loading programs…") {
            val list = Parse.tools(Api.arr(app, "/v1/tools"))
            ui {
                if (g != gen) return@ui
                status.text = "Programs the server can download; tap one to see what would be downloaded"
                showRows(list.map { t -> Row(toolLabel(t), { toolTapped(t) }) })
            }
            // keep the progress current while something downloads
            if (list.any { it.busy }) { Thread.sleep(1500); ui { if (g == gen && view == "tools") tools() } }
        }
    }

    private fun toolLabel(t: ToolInfo): String {
        val state = when {
            t.busy -> "${t.phase}${if (t.phase == "downloading") " ${t.percent}%" else "…"}"
            t.found == "installed" -> "installed by Mus-Eq"
            t.found == "modified" -> "installed copy was changed, so it is not used"
            t.found == "configured" -> "using the program you configured"
            t.found == "system" -> "found on the server"
            t.phase == "failed" -> "not installed (last try failed: ${t.error})"
            else -> "not installed"
        }
        return "${t.name} ${t.version} · $state"
    }

    private fun toolTapped(t: ToolInfo) {
        if (t.busy) return
        val b = android.app.AlertDialog.Builder(this)
        if (t.found == "installed" || t.found == "modified") {
            b.setTitle("Remove ${t.name}?")
                .setMessage("This removes the copy Mus-Eq installed on the server. The server can download it again later.")
                .setPositiveButton("Remove") { _, _ -> removeTool(t) }
        } else if (t.found == "missing" && t.supported && t.message != null) {
            // the whole message is shown first; the confirmation phrase is sent only by this button
            b.setTitle("Download ${t.name}?")
                .setMessage(t.message)
                .setPositiveButton("Download and install") { _, _ -> installTool(t) }
        } else {
            b.setTitle(t.name).setMessage(
                if (!t.supported) "This release has no download for the server's system. Install it yourself and set its environment variable (for example MUSEQ_FFMPEG)."
                else "The server already has ${t.name} (${t.found}); nothing to download.")
        }
        b.setNegativeButton("Close", null).show()
    }

    private fun installTool(t: ToolInfo) {
        val app = applicationContext
        background("starting the download…") {
            val (code, body) = Api.send(app, "POST", "/v1/tools/${Api.enc(t.name)}/install", JSONObject().put("confirm", t.confirmPhrase).toString())
            if (code != 202) throw RuntimeException(Parse.errorText(body, "HTTP $code"))
            ui { if (view == "tools") tools() }
        }
    }

    private fun removeTool(t: ToolInfo) {
        val app = applicationContext
        background("removing ${t.name}…") {
            val (code, body) = Api.send(app, "DELETE", "/v1/tools/${Api.enc(t.name)}")
            if (code != 204) throw RuntimeException(Parse.errorText(body, "HTTP $code"))
            ui { if (view == "tools") tools() }
        }
    }

    // ---- radio and stations -----------------------------------------------------------------

    private fun radios() {
        val app = applicationContext
        val g = gen
        background("loading radios…") {
            val a = Api.arr(app, "/v1/radios")
            ui {
                if (g != gen) return@ui
                status.text = "${a.length()} library radios; tap to listen"
                showRows((0 until a.length()).map { a.getJSONObject(it) }.map { r ->
                    Row("${r.getString("name")} · ${r.getString("codec")} ${r.getInt("kbps")} k · ${r.getInt("listeners")} listening",
                        { playStream(r.getString("name"), Prefs.base(this) + r.getString("listen_path")) })
                })
            }
        }
    }

    private fun stations(q: String) {
        val app = applicationContext
        val g = gen
        background("loading stations…") {
            val j = Api.obj(app, "/v1/stations?limit=200&q=" + Api.enc(q))
            val a = j.getJSONArray("items")
            ui {
                if (g != gen) return@ui
                status.text = "${a.length()} of ${j.getInt("total")} stations; tap to listen"
                showRows((0 until a.length()).map { a.getJSONObject(it) }.map { s ->
                    val alive = if (s.isNull("alive")) "" else if (s.getBoolean("alive")) "" else " (down?)"
                    Row("${s.getString("name")} · ${s.getString("category")}$alive", { playStream(s.getString("name"), s.getString("url")) })
                })
            }
        }
    }

    // ---- playing --------------------------------------------------------------------------

    // the queue names a track as museq://track/<fileId>; the service picks the pinned file or the server
    // when that track opens, so unpinning a queued track does not break the queue
    private fun uriOf(t: Track): Uri = Uri.parse("museq://track/" + t.fileId)

    private fun controller(then: (MediaController) -> Unit) {
        val f = controllerFuture ?: MediaController.Builder(this,
            SessionToken(this, ComponentName(this, PlaybackService::class.java))).buildAsync()
            .also { fut -> controllerFuture = fut
                fut.addListener({ runCatching { fut.get() }.getOrNull()?.addListener(object : Player.Listener {
                    override fun onPlayerError(e: PlaybackException) { ui { status.text = "playback error: " + e.errorCodeName } }
                }) }, MoreExecutors.directExecutor()) }
        f.addListener({ runCatching { f.get() }.getOrNull()?.let(then) }, MoreExecutors.directExecutor())
    }

    private fun play(tapped: Track) {
        // the queue is the whole visible track list; a track that is neither pinned nor reachable stops
        // playback with an error shown in the status line
        val start = tracks.indexOf(tapped)
        val items = tracks.map {
            MediaItem.Builder().setMediaId(it.id).setUri(uriOf(it))
                .setMediaMetadata(MediaMetadata.Builder().setTitle(it.title).setArtist(it.artist).build()).build()
        }
        controller { it.setMediaItems(items, start, 0); it.prepare(); it.play() }
    }

    /** A radio or station: one endless stream. */
    private fun playStream(name: String, url: String) {
        val item = MediaItem.Builder().setUri(url)
            .setMediaMetadata(MediaMetadata.Builder().setTitle(name).build()).build()
        controller { it.setMediaItems(listOf(item)); it.prepare(); it.play() }
    }
}
