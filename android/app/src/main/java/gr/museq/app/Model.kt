// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
package gr.museq.app

import org.json.JSONObject
import java.io.File
import java.security.MessageDigest

class Track(val id: String, val title: String, val artist: String, val fileId: String)

/** One track as the server's manifest describes it: the details plus size and SHA-256 of the file that plays. */
class Entry(val track: Track, val bytes: Long, val sha256: String)

/** Parsing of what the server sends. Kept free of Android classes so it can be unit tested. */
object Parse {
    fun track(o: JSONObject) = Track(
        o.getString("id"), o.getString("title"),
        if (o.isNull("artist")) "" else o.getString("artist"), o.getString("preferred_file_id"))

    fun manifest(j: JSONObject): Pair<List<Entry>, List<String>> {
        val a = j.getJSONArray("items")
        val items = (0 until a.length()).map {
            val o = a.getJSONObject(it)
            Entry(track(o.getJSONObject("track")), o.getLong("bytes"), o.getString("sha256"))
        }
        val m = j.getJSONArray("missing")
        return items to (0 until m.length()).map { m.getString(it) }
    }

    private val range = Regex("bytes (\\d+)-(\\d+)/(\\d+|\\*)")
    private val unsatisfied = Regex("bytes \\*/(\\d+)")

    /** First byte of a `Content-Range: bytes a-b/total` header, or null if it is missing or not that shape. */
    fun rangeStart(h: String?): Long? = h?.let { range.find(it)?.groupValues?.get(1)?.toLongOrNull() }

    // Size named by a 416 answer's Content-Range header (the form with a star instead of a byte range).
    fun unsatisfiedTotal(h: String?): Long? = h?.let { unsatisfied.find(it)?.groupValues?.get(1)?.toLongOrNull() }

    fun sizeLabel(done: Long, total: Long) =
        if (total >= 0) "${done / 1048576} of ${total / 1048576} MB" else "${done / 1048576} MB"
}

fun sha256Hex(f: File): String {
    val md = MessageDigest.getInstance("SHA-256")
    f.inputStream().use { ins ->
        val buf = ByteArray(64 * 1024)
        while (true) { val n = ins.read(buf); if (n < 0) break; md.update(buf, 0, n) }
    }
    return md.digest().joinToString("") { "%02x".format(it) }
}
