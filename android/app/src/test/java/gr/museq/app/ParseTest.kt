// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
package gr.museq.app

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.io.File

class ParseTest {
    private val manifest = """
        {"items":[{"track":{"id":"7","title":"Song","artist":null,"album":null,"duration_ms":1000,
                            "preferred_file_id":"12","genre":null},
                   "file_id":"12","bytes":5,"sha256":"abc"}],
         "missing":["99"]}"""

    @Test fun manifestKeepsTrackFileSizeAndHash() {
        val (items, missing) = Parse.manifest(JSONObject(manifest))
        assertEquals(listOf("99"), missing)
        val e = items.single()
        assertEquals("7", e.track.id); assertEquals("12", e.track.fileId)
        assertEquals("", e.track.artist) // a JSON null becomes an empty artist, never the text "null"
        assertEquals(5L, e.bytes); assertEquals("abc", e.sha256)
    }

    @Test fun rangeStartReadsTheFirstByteOrNothing() {
        assertEquals(100L, Parse.rangeStart("bytes 100-199/200"))
        assertEquals(0L, Parse.rangeStart("bytes 0-9/*"))
        assertNull(Parse.rangeStart(null))
        assertNull(Parse.rangeStart("items 1-2/3"))
        assertNull(Parse.rangeStart("bytes */200")) // the 416 form has no start
    }

    @Test fun unsatisfiedTotalReadsOnlyTheStarForm() {
        assertEquals(114170338L, Parse.unsatisfiedTotal("bytes */114170338"))
        assertNull(Parse.unsatisfiedTotal("bytes 0-9/200"))
        assertNull(Parse.unsatisfiedTotal(null))
    }

    @Test fun sizeLabelHandlesUnknownTotals() {
        assertEquals("1 of 3 MB", Parse.sizeLabel(1048576, 3 * 1048576))
        assertEquals("2 MB", Parse.sizeLabel(2 * 1048576, -1))
    }

    @Test fun onlyPlainIdsCanBecomeFileNames() {
        assertTrue(Pins.usable("12")); assertTrue(Pins.usable("a-b_C9"))
        for (bad in listOf("", "../x", "a/b", "a.b", "a b", "a\u0000b", "x".repeat(129))) assertFalse(bad, Pins.usable(bad))
    }

    @Test fun sha256OfAKnownFile() {
        val f = File.createTempFile("museq", ".bin"); f.deleteOnExit()
        f.writeText("abc")
        assertEquals("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", sha256Hex(f))
    }

    private fun toolJson(phase: String, message: String) = """
        {"name":"ffmpeg","purpose":"to convert tracks","version":"n8.1.3","licence":"LGPL","homepage":"x","notes":"",
         "platform_supported":true,"source_url":"https://github.com/a","download_bytes":135188284,"sha256":"ab",
         "found":"missing","path":null,"message":$message,"confirm_phrase":"install ffmpeg n8.1.3",
         "progress":{"phase":"$phase","downloaded":50,"total":200,"error":null}}"""

    @Test fun toolInfoKeepsTheConfirmationPhraseAndProgress() {
        val t = Parse.tool(JSONObject(toolJson("downloading", "\"Mus-Eq needs ffmpeg. Confirm by sending: install ffmpeg n8.1.3\"")))
        assertEquals("install ffmpeg n8.1.3", t.confirmPhrase)
        assertTrue(t.message!!.contains("Confirm by sending"))
        assertTrue(t.busy); assertEquals(25, t.percent); assertNull(t.error)
        val idle = Parse.tool(JSONObject(toolJson("idle", "null")))
        assertFalse(idle.busy); assertNull(idle.message)
    }

    @Test fun serverErrorTextIsReadOrFallsBack() {
        assertEquals("not confirmed. Mus-Eq needs ffmpeg", Parse.errorText("""{"error":"not confirmed. Mus-Eq needs ffmpeg"}""", "HTTP 400"))
        assertEquals("HTTP 502", Parse.errorText("<html>bad gateway</html>", "HTTP 502"))
        assertEquals("HTTP 400", Parse.errorText("", "HTTP 400"))
    }
}
