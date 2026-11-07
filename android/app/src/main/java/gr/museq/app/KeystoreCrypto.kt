// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
package gr.museq.app

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * AES-GCM with a key held by the Android Keystore. The key does not require the user to be present (no fingerprint,
 * no unlocked screen): the media service reads the token to open the next track while the phone is locked.
 * The blob is `base64(iv):base64(ciphertext)`.
 */
class KeystoreCrypto : Crypto {
    private fun keyStore(): KeyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    /** The key if there is one. An entry that exists but cannot be read is removed: nothing encrypted with it is usable. */
    @Synchronized
    private fun existing(): SecretKey? {
        val ks = keyStore()
        return try {
            ks.getKey(ALIAS, null) as? SecretKey
        } catch (e: java.security.UnrecoverableKeyException) {
            try { ks.deleteEntry(ALIAS) } catch (_: Exception) {}
            null
        }
    }

    /** The key, created if there is none. Serialised, so two threads cannot both create one. */
    @Synchronized
    private fun keyOrNew(): SecretKey {
        existing()?.let { return it }
        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        gen.init(
            KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build())
        return gen.generateKey()
    }

    override fun encrypt(plain: String): String {
        return try {
            encryptWith(keyOrNew(), plain)
        } catch (e: java.security.InvalidKeyException) {
            // a key the key store will not use (some vendor builds do this): replace it once and try again. Anything
            // encrypted with the old key is lost, which the store already handles by asking for the token again.
            try { keyStore().deleteEntry(ALIAS) } catch (_: Exception) {}
            encryptWith(keyOrNew(), plain)
        }
    }

    private fun encryptWith(key: SecretKey, plain: String): String {
        val c = Cipher.getInstance("AES/GCM/NoPadding")
        c.init(Cipher.ENCRYPT_MODE, key)
        val ct = c.doFinal(plain.toByteArray(Charsets.UTF_8))
        return b64(c.iv) + ":" + b64(ct)
    }

    /**
     * [UnusableBlob] only for what proves the blob can never be read: a malformed blob, no key, or a wrong key
     * (the GCM tag does not verify). Anything else the key store throws propagates as a failure that may pass.
     */
    override fun decrypt(blob: String): String {
        val parts = blob.split(":")
        if (parts.size != 2) throw UnusableBlob("malformed blob")
        val iv: ByteArray
        val ct: ByteArray
        try {
            iv = Base64.decode(parts[0], Base64.NO_WRAP)
            ct = Base64.decode(parts[1], Base64.NO_WRAP)
        } catch (e: IllegalArgumentException) {
            throw UnusableBlob("malformed blob", e)
        }
        val key = existing() ?: throw UnusableBlob("the key is gone")
        val c = Cipher.getInstance("AES/GCM/NoPadding")
        c.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, iv))
        try {
            return String(c.doFinal(ct), Charsets.UTF_8)
        } catch (e: javax.crypto.BadPaddingException) { // includes AEADBadTagException
            throw UnusableBlob("wrong key or damaged blob", e)
        }
    }

    private fun b64(b: ByteArray) = Base64.encodeToString(b, Base64.NO_WRAP)

    companion object { private const val ALIAS = "museq_owner_token" }
}
