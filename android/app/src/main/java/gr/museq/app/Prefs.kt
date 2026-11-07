// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
package gr.museq.app

import android.content.Context

/** Server address and owner token. Stored in app-private preferences (not encrypted). */
object Prefs {
    private fun p(c: Context) = c.getSharedPreferences("museq", Context.MODE_PRIVATE)
    fun base(c: Context): String = p(c).getString("base", "") ?: ""
    fun token(c: Context): String = p(c).getString("token", "") ?: ""
    fun save(c: Context, base: String, token: String) =
        p(c).edit().putString("base", base.trim().trimEnd('/')).putString("token", token.trim()).apply()
}
