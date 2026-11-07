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
    private fun dir(c: Context) = File(c.filesDir, "pins").apply { mkdirs() }
    private fun index(c: Context) = File(dir(c), "index.json")

    fun file(c: Context, fileId: String) = File(dir(c), safe(fileId))
    fun isPinned(c: Context, fileId: String) = file(c, fileId).isFile

    // file ids come from the server; never let one name a path outside the pin folder
    private fun safe(id: String) = id.filter { it.isLetterOrDigit() || it == '-' || it == '_' }.ifEmpty { "x" }

    @Synchronized
    fun all(c: Context): List<Track> = try {
        val a = JSONArray(index(c).readText())
        (0 until a.length()).map { val o = a.getJSONObject(it)
            Track(o.getString("id"), o.getString("title"), o.optString("artist", ""), o.getString("fileId")) }
            .filter { isPinned(c, it.fileId) }
    } catch (e: Exception) { emptyList() }

    @Synchronized
    private fun write(c: Context, list: List<Track>) {
        val a = JSONArray(); list.forEach { a.put(JSONObject().put("id", it.id).put("title", it.title)
            .put("artist", it.artist).put("fileId", it.fileId)) }
        index(c).writeText(a.toString())
    }

    /** Downloads the file, resuming a partial one. Returns null on success or an error text. */
    fun pin(c: Context, t: Track, progress: (Long, Long) -> Unit): String? {
        val part = File(dir(c), safe(t.fileId) + ".part")
        try {
            val have = if (part.isFile) part.length() else 0L
            val u = URL(Prefs.base(c) + "/v1/audio/" + t.fileId)
            val cn = u.openConnection() as HttpURLConnection
            cn.setRequestProperty("Authorization", "Bearer " + Prefs.token(c))
            if (have > 0) cn.setRequestProperty("Range", "bytes=$have-")
            cn.connectTimeout = 8000; cn.readTimeout = 20000
            val code = cn.responseCode
            if (code == 416) { part.delete(); return "range refused, retry" }
            if (code != 200 && code != 206) return "HTTP $code"
            val resumed = code == 206
            val total = (if (resumed) have else 0L) + cn.contentLengthLong
            RandomAccessFile(part, "rw").use { f ->
                if (resumed) f.seek(have) else f.setLength(0)
                cn.inputStream.use { ins ->
                    val buf = ByteArray(64 * 1024); var done = if (resumed) have else 0L
                    while (true) { val n = ins.read(buf); if (n < 0) break
                        f.write(buf, 0, n); done += n; progress(done, total) }
                }
            }
            if (part.length() != total) return "incomplete (${part.length()} of $total), retry resumes"
            if (!part.renameTo(file(c, t.fileId))) return "could not store the file"
            write(c, all(c).filter { it.fileId != t.fileId } + t)
            return null
        } catch (e: Exception) { return e.message ?: e.javaClass.simpleName }
    }

    fun unpin(c: Context, t: Track) {
        file(c, t.fileId).delete()
        File(dir(c), safe(t.fileId) + ".part").delete()
        write(c, all(c).filter { it.fileId != t.fileId })
    }
}
