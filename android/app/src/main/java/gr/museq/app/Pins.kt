package gr.museq.app

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.RandomAccessFile
import java.net.HttpURLConnection
import java.net.URL

/** Offline pins: files are kept in app storage and resumed from a `.part` file if the download is cut. */
object Pins {
    private val ID = Regex("^[A-Za-z0-9_-]{1,128}$")
    private val inFlight = HashSet<String>()

    private fun dir(c: Context) = File(c.filesDir, "pins").apply { mkdirs() }
    private fun index(c: Context) = File(dir(c), "index.json")

    /** Ids that are not plain `[A-Za-z0-9_-]` are refused, so two ids can never share a file. */
    fun usable(fileId: String) = ID.matches(fileId)
    fun file(c: Context, fileId: String) = File(dir(c), fileId)
    fun isPinned(c: Context, fileId: String) = usable(fileId) && file(c, fileId).isFile
    @Synchronized fun isBusy(fileId: String) = inFlight.contains(fileId)

    @Synchronized
    fun all(c: Context): List<Track> = try {
        val a = JSONArray(index(c).readText())
        (0 until a.length()).map { val o = a.getJSONObject(it)
            Track(o.getString("id"), o.getString("title"), o.optString("artist", ""), o.getString("fileId")) }
            .filter { isPinned(c, it.fileId) }
    } catch (e: Exception) { emptyList() }

    private fun write(c: Context, list: List<Track>) {
        val a = JSONArray(); list.forEach { a.put(JSONObject().put("id", it.id).put("title", it.title)
            .put("artist", it.artist).put("fileId", it.fileId)) }
        val tmp = File(dir(c), "index.json.tmp")
        tmp.writeText(a.toString()); tmp.renameTo(index(c))
    }

    /** The whole read-change-write of the index is one step, so two finishing downloads cannot drop each other. */
    @Synchronized
    private fun addToIndex(c: Context, t: Track) = write(c, all(c).filter { it.fileId != t.fileId } + t)

    @Synchronized
    private fun removeFromIndex(c: Context, t: Track) = write(c, all(c).filter { it.fileId != t.fileId })

    /** Downloads the file, resuming a partial one. Returns null on success or an error text. */
    fun pin(c: Context, e: Entry, progress: (Long, Long) -> Unit): String? {
        val t = e.track
        if (!usable(t.fileId)) return "file id not usable offline"
        synchronized(this) { if (!inFlight.add(t.fileId)) return "already downloading" }
        try { return download(c, e, progress) } finally { synchronized(this) { inFlight.remove(t.fileId) } }
    }

    /** True if the stored copy has the size and SHA-256 the server's manifest names. */
    fun matches(c: Context, e: Entry): Boolean {
        val f = file(c, e.track.fileId)
        return usable(e.track.fileId) && f.isFile && f.length() == e.bytes && sha256Hex(f) == e.sha256
    }

    private fun download(c: Context, e: Entry, progress: (Long, Long) -> Unit): String? {
        val t = e.track
        val part = File(dir(c), t.fileId + ".part")
        var cn: HttpURLConnection? = null
        try {
            var have = if (part.isFile) part.length() else 0L
            val u = URL(Prefs.base(c) + "/v1/audio/" + t.fileId)
            cn = u.openConnection() as HttpURLConnection
            cn.instanceFollowRedirects = false // the token must never follow a redirect to another host
            cn.setRequestProperty("Authorization", "Bearer " + Prefs.token(c))
            cn.setRequestProperty("Accept-Encoding", "identity") // keeps Content-Length truthful
            if (have > 0) cn.setRequestProperty("Range", "bytes=$have-")
            cn.connectTimeout = 8000; cn.readTimeout = 20000
            val code = cn.responseCode
            if (code == 416) {
                // "bytes */N": if N is the size of what we already hold, the earlier download had finished
                if (have > 0 && Parse.unsatisfiedTotal(cn.getHeaderField("Content-Range")) == have) return finish(c, e, part)
                part.delete(); return "range refused, retry"
            }
            if (code != 200 && code != 206) return "HTTP $code"
            var resumed = code == 206
            if (resumed) {
                // the server must continue exactly where our partial file ends
                if (Parse.rangeStart(cn.getHeaderField("Content-Range")) != have) { part.delete(); return "resume offset mismatch, retry" }
            }
            if (!resumed) have = 0L
            val len = cn.contentLengthLong
            val total = if (len >= 0) have + len else -1L
            RandomAccessFile(part, "rw").use { f ->
                if (resumed) f.seek(have) else f.setLength(0)
                cn.inputStream.use { ins ->
                    val buf = ByteArray(64 * 1024); var done = have
                    while (true) { val n = ins.read(buf); if (n < 0) break
                        f.write(buf, 0, n); done += n; progress(done, total) }
                }
            }
            // with a known length the size must match; with an unknown one, a clean end of stream is all there is
            if (total >= 0 && part.length() != total) return "incomplete (${part.length()} of $total), retry resumes"
            return finish(c, e, part)
        } catch (e: Exception) { return e.message ?: e.javaClass.simpleName }
        finally { cn?.disconnect() }
    }

    /** The partial file becomes the pin only if it has the size and hash the manifest promised. */
    private fun finish(c: Context, e: Entry, part: File): String? {
        if (part.length() != e.bytes) { part.delete(); return "size differs from the server's (${part.length()} of ${e.bytes}), retry" }
        if (sha256Hex(part) != e.sha256) { part.delete(); return "checksum differs from the server's, retry" }
        if (!part.renameTo(file(c, e.track.fileId))) return "could not store the file"
        addToIndex(c, e.track)
        return null
    }

    fun unpin(c: Context, t: Track) {
        if (!usable(t.fileId) || isBusy(t.fileId)) return
        file(c, t.fileId).delete()
        File(dir(c), t.fileId + ".part").delete()
        removeFromIndex(c, t)
    }
}
