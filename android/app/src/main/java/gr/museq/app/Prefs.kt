// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
package gr.museq.app

import android.content.Context
import android.content.SharedPreferences

/** Server address (app-private preferences) and the owner token (encrypted with a Keystore key, see TokenStore). */
object Prefs {
    private fun p(c: Context) = c.getSharedPreferences("museq", Context.MODE_PRIVATE)

    private class PrefsKv(private val p: SharedPreferences) : Kv {
        override fun get(key: String): String? = p.getString(key, null)
        override fun put(key: String, value: String) = p.edit().putString(key, value).apply()
        override fun remove(key: String) = p.edit().remove(key).apply()
    }

    @Volatile private var store: TokenStore? = null
    private fun store(c: Context): TokenStore =
        store ?: synchronized(this) { store ?: TokenStore(PrefsKv(p(c.applicationContext)), KeystoreCrypto()).also { store = it } }

    fun base(c: Context): String = p(c).getString("base", "") ?: ""
    fun token(c: Context): String = store(c).get()

    /**
     * Saves the token first and the address only if that worked, so a failure leaves both as they were. Returns false
     * if the token could not be stored securely.
     */
    fun save(c: Context, base: String, token: String): Boolean {
        try { store(c).setIfChanged(token) } catch (e: Exception) { return false }
        p(c).edit().putString("base", base.trim().trimEnd('/')).apply()
        return true
    }
}
