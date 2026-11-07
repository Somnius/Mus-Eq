package gr.museq.app

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder

/** Small blocking HTTP helper for the server's JSON routes. Call it off the UI thread. */
object Api {
    fun enc(s: String): String = URLEncoder.encode(s, "UTF-8")

    fun text(c: Context, path: String): String {
        val cn = URL(Prefs.base(c) + path).openConnection() as HttpURLConnection
        try {
            cn.instanceFollowRedirects = false // the token must not follow a redirect to another host
            cn.setRequestProperty("Authorization", "Bearer " + Prefs.token(c))
            cn.connectTimeout = 8000; cn.readTimeout = 15000
            if (cn.responseCode != 200) throw RuntimeException("HTTP " + cn.responseCode)
            return cn.inputStream.bufferedReader().readText()
        } finally { cn.disconnect() }
    }

    fun obj(c: Context, path: String) = JSONObject(text(c, path))
    fun arr(c: Context, path: String) = JSONArray(text(c, path))

    /** Manifest entries for track ids, in chunks the server accepts. */
    fun manifest(c: Context, ids: List<String>): Pair<List<Entry>, List<String>> {
        val items = ArrayList<Entry>(); val missing = ArrayList<String>()
        for (chunk in ids.chunked(200)) {
            val (i, m) = Parse.manifest(obj(c, "/v1/tracks/manifest?ids=" + chunk.joinToString(",")))
            items += i; missing += m
        }
        return items to missing
    }
}
