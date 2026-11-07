package gr.museq.app

import android.Manifest
import android.content.ComponentName
import android.os.Bundle
import android.widget.*
import android.app.Activity
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import com.google.common.util.concurrent.MoreExecutors
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder
import kotlin.concurrent.thread

class Track(val id: String, val title: String, val artist: String, val fileId: String)

class MainActivity : Activity() {
    private var controller: MediaController? = null
    private var tracks = listOf<Track>()
    private lateinit var status: TextView
    private lateinit var list: ListView

    override fun onCreate(b: Bundle?) {
        super.onCreate(b)
        if (android.os.Build.VERSION.SDK_INT >= 33) requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        val root = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL; setPadding(24, 48, 24, 24) }
        val url = EditText(this).apply { inputType = android.text.InputType.TYPE_CLASS_TEXT or android.text.InputType.TYPE_TEXT_VARIATION_URI; hint = "http://10.0.0.3:8096"; setText(Prefs.base(context)) }
        val tok = EditText(this).apply { inputType = android.text.InputType.TYPE_CLASS_TEXT or android.text.InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD or android.text.InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS; hint = "owner token"; setText(Prefs.token(context)) }
        val q = EditText(this).apply { hint = "search" }
        val go = Button(this).apply { text = "Connect and search" }
        status = TextView(this)
        list = ListView(this)
        listOf(url, tok, q, go, status, list).forEach { root.addView(it) }
        setContentView(root)
        go.setOnClickListener {
            Prefs.save(this, url.text.toString(), tok.text.toString())
            search(q.text.toString())
        }
        list.setOnItemClickListener { _, _, i, _ -> play(i) }
        list.setOnItemLongClickListener { _, _, i, _ -> togglePin(i); true }
    }

    private fun search(q: String) {
        status.text = "searching…"
        thread {
            try {
                val u = URL(Prefs.base(this) + "/v1/tracks?limit=200&q=" + URLEncoder.encode(q, "UTF-8"))
                val c = u.openConnection() as HttpURLConnection
                c.setRequestProperty("Authorization", "Bearer " + Prefs.token(this))
                c.connectTimeout = 8000; c.readTimeout = 15000
                if (c.responseCode != 200) throw RuntimeException("HTTP " + c.responseCode)
                val j = JSONObject(c.inputStream.bufferedReader().readText())
                val a = j.getJSONArray("items")
                tracks = (0 until a.length()).map {
                    val o = a.getJSONObject(it)
                    Track(o.getString("id"), o.getString("title"),
                        o.optString("artist", ""), o.getString("preferred_file_id"))
                }
                runOnUiThread {
                    status.text = "${tracks.size} of ${j.getInt("total")} tracks"
                    show(tracks)
                }
            } catch (e: Exception) {
                runOnUiThread { status.text = "offline or failed (" + e.message + "), showing pinned"; show(Pins.all(this)) }
            }
        }
    }

    private fun label(t: Track) = (if (Pins.isPinned(this, t.fileId)) "[offline] " else "") +
        (if (t.artist.isEmpty() || t.artist == "null") "" else t.artist + " – ") + t.title

    private fun show(l: List<Track>) {
        tracks = l
        list.adapter = ArrayAdapter(this, android.R.layout.simple_list_item_1, l.map { label(it) })
    }

    private fun togglePin(i: Int) {
        val t = tracks[i]
        if (Pins.isPinned(this, t.fileId)) { Pins.unpin(this, t); status.text = "unpinned"; show(tracks); return }
        status.text = "pinning…"
        thread {
            var last = 0L
            val err = Pins.pin(this, t) { d, tot ->
                if (System.currentTimeMillis() - last > 500) { last = System.currentTimeMillis()
                    runOnUiThread { status.text = "pinning ${d / 1048576} of ${tot / 1048576} MB" } } }
            runOnUiThread { status.text = if (err == null) "pinned" else "pin failed: $err"; show(tracks) }
        }
    }

    private fun play(start: Int) {
        val items = tracks.map {
            MediaItem.Builder().setMediaId(it.id)
                .setUri(if (Pins.isPinned(this, it.fileId)) android.net.Uri.fromFile(Pins.file(this, it.fileId))
                        else android.net.Uri.parse(Prefs.base(this) + "/v1/audio/" + it.fileId))
                .setMediaMetadata(MediaMetadata.Builder().setTitle(it.title).setArtist(it.artist).build())
                .build()
        }
        val start0 = {
            controller?.run { setMediaItems(items, start, 0); prepare(); play() }
        }
        if (controller != null) { start0(); return }
        val f = MediaController.Builder(this, SessionToken(this, ComponentName(this, PlaybackService::class.java))).buildAsync()
        f.addListener({ controller = f.get(); start0() }, MoreExecutors.directExecutor())
    }
}
