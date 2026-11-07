// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
package gr.museq.app

/** Where the token's text is kept. Kept apart from Android so the logic below can be tested on a plain JVM. */
interface Kv {
    fun get(key: String): String?
    fun put(key: String, value: String)
    fun remove(key: String)
}

/** Thrown by `Crypto.decrypt` only when the blob can never be decrypted (damaged, malformed, or its key is gone). */
class UnusableBlob(message: String, cause: Throwable? = null) : Exception(message, cause)

/**
 * Encrypts and decrypts the token. `decrypt` must throw [UnusableBlob] when the blob is provably unusable, and any
 * other exception for a failure that may pass (the key store busy or restarting): the store keeps the blob then.
 */
interface Crypto {
    fun encrypt(plain: String): String
    fun decrypt(blob: String): String
}

/**
 * The owner token, stored encrypted. Rules:
 *  - the plaintext copy is removed only after the encrypted one has been read back and matches, so a failing key
 *    store never loses the token (the plaintext stays and is used until the next successful save);
 *  - the encrypted copy is dropped only when it is provably unusable ([UnusableBlob]: key deleted, storage restored
 *    on another phone). A failure that may pass keeps it, is not cached, and is retried on the next read;
 *  - when an encrypted and a plaintext copy are both on disk (the process died between the two writes of a
 *    migration), the plaintext goes only once the encrypted copy has been decrypted, and is used to rebuild the
 *    encrypted one if that cannot be;
 *  - the decrypted token is kept in memory, so the playback service does not touch the key store for every track;
 *  - reading and writing are serialised, so a stale read can never overwrite a newer save in the cache.
 */
class TokenStore(
    private val kv: Kv,
    private val crypto: Crypto,
    private val now: () -> Long = System::currentTimeMillis,
) {
    @Volatile private var cache: String? = null
    private var migrateAfter = 0L // a failed migration is tried again after a pause, not on every read

    @Synchronized
    fun get(): String {
        cache?.let { return it }
        val blob = kv.get(ENC)
        val plain = kv.get(PLAIN)?.takeIf { it.isNotEmpty() }
        if (blob != null) {
            try {
                val t = crypto.decrypt(blob)
                if (kv.get(PLAIN) != null) kv.remove(PLAIN) // the encrypted copy is proven readable
                cache = t
                return t
            } catch (e: UnusableBlob) {
                kv.remove(ENC) // gone for good; fall through to a plaintext copy if there is one
            } catch (e: Exception) {
                return plain ?: "" // may pass: change nothing, cache nothing, try again next time
            }
        }
        if (plain == null) {
            if (kv.get(PLAIN) != null) kv.remove(PLAIN)
            cache = ""
            return ""
        }
        // a plaintext token (an old install, or one that could not be encrypted): encrypt it, or keep using it
        if (now() < migrateAfter) return plain
        try {
            set(plain)
        } catch (e: Exception) {
            migrateAfter = now() + RETRY_MS // not cached: the next read after the pause tries again
        }
        return plain
    }

    /** Like [set], but a token equal to the one already stored is left alone (no new encryption, nothing can fail). */
    @Synchronized
    fun setIfChanged(token: String) {
        if (token.trim() != get()) set(token)
    }

    /** Stores the token encrypted; an empty token clears it. Throws if it cannot be stored safely. */
    @Synchronized
    fun set(token: String) {
        val t = token.trim()
        if (t.isEmpty()) {
            kv.remove(ENC); kv.remove(PLAIN); cache = ""
            return
        }
        val blob = crypto.encrypt(t)
        check(crypto.decrypt(blob) == t) { "the encrypted token did not read back" }
        kv.put(ENC, blob)
        kv.remove(PLAIN)
        cache = t
    }

    companion object {
        const val PLAIN = "token"
        const val ENC = "token_enc"
        const val RETRY_MS = 60_000L
    }
}
