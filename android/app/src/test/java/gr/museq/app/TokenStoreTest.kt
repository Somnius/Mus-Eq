// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
package gr.museq.app

import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread

class TokenStoreTest {
    private class FakeKv(val m: MutableMap<String, String> = mutableMapOf()) : Kv {
        override fun get(key: String) = m[key]
        override fun put(key: String, value: String) { m[key] = value }
        override fun remove(key: String) { m.remove(key) }
    }

    /** A reversible fake "encryption", with switches to make it fail the way a key store can. */
    private class FakeCrypto(
        var failEncrypt: Boolean = false,
        var keyGone: Boolean = false,          // decrypt proves the blob is unusable
        var transientFailures: Int = 0,        // decrypt fails this many times with an error that may pass
        var garbleRead: Boolean = false,
    ) : Crypto {
        var decrypts = 0
        override fun encrypt(plain: String): String { if (failEncrypt) throw IllegalStateException("no key"); return "enc(" + plain.reversed() + ")" }
        override fun decrypt(blob: String): String {
            decrypts++
            if (keyGone) throw UnusableBlob("key gone")
            if (transientFailures > 0) { transientFailures--; throw IllegalStateException("keystore busy") }
            val inner = blob.removePrefix("enc(").removeSuffix(")").reversed()
            return if (garbleRead) inner + "x" else inner
        }
    }

    @Test fun aTokenIsStoredEncryptedAndReadBack() {
        val kv = FakeKv(); val s = TokenStore(kv, FakeCrypto())
        s.set("  secret-token  ")
        assertEquals("secret-token", s.get())
        assertEquals("enc(nekot-terces)", kv.get(TokenStore.ENC))
        assertNull("no plaintext copy is kept", kv.get(TokenStore.PLAIN))
        assertEquals("secret-token", TokenStore(kv, FakeCrypto()).get()) // the app restarted
    }

    @Test fun anOldPlaintextTokenIsMigratedAndTheCopyRemoved() {
        val kv = FakeKv(mutableMapOf(TokenStore.PLAIN to "old-token"))
        assertEquals("old-token", TokenStore(kv, FakeCrypto()).get())
        assertNull(kv.get(TokenStore.PLAIN))
        assertNotNull(kv.get(TokenStore.ENC))
    }

    @Test fun aFailingKeyStoreNeverLosesTheToken() {
        val kv = FakeKv(mutableMapOf(TokenStore.PLAIN to "old-token"))
        assertEquals("old-token", TokenStore(kv, FakeCrypto(failEncrypt = true)).get())
        assertEquals("old-token", kv.get(TokenStore.PLAIN))
        assertNull(kv.get(TokenStore.ENC))
        // a copy that does not read back identically is refused, and the plaintext is not deleted
        val kv2 = FakeKv(mutableMapOf(TokenStore.PLAIN to "tok"))
        TokenStore(kv2, FakeCrypto(garbleRead = true)).get()
        assertEquals("tok", kv2.get(TokenStore.PLAIN))
        assertNull(kv2.get(TokenStore.ENC))
    }

    @Test fun aProvablyLostKeyMeansAskingForTheTokenAgainNotACrash() {
        val kv = FakeKv(); TokenStore(kv, FakeCrypto()).set("tok")
        assertEquals("", TokenStore(kv, FakeCrypto(keyGone = true)).get())
        assertNull("the unusable copy is dropped", kv.get(TokenStore.ENC))
        TokenStore(kv, FakeCrypto()).also { it.set("new"); assertEquals("new", it.get()) }
    }

    @Test fun aFailureThatMayPassKeepsTheStoredTokenAndIsRetried() {
        val kv = FakeKv(); TokenStore(kv, FakeCrypto()).set("tok")
        val crypto = FakeCrypto(transientFailures = 1)
        val s = TokenStore(kv, crypto)
        assertEquals("", s.get())                                  // this read could not decrypt
        assertEquals("enc(kot)", kv.get(TokenStore.ENC))           // but nothing was deleted
        assertEquals("tok", s.get())                               // and the next read works
        assertEquals(2, crypto.decrypts)
    }

    @Test fun bothCopiesOnDiskAfterAKilledMigration() {
        // the encrypted copy reads back: the plaintext goes
        val kv = FakeKv(mutableMapOf(TokenStore.PLAIN to "tok", TokenStore.ENC to "enc(kot)"))
        assertEquals("tok", TokenStore(kv, FakeCrypto()).get())
        assertNull(kv.get(TokenStore.PLAIN))
        // the encrypted copy cannot be read: the plaintext survives and rebuilds it
        val kv2 = FakeKv(mutableMapOf(TokenStore.PLAIN to "tok", TokenStore.ENC to "garbage"))
        val c2 = object : Crypto {
            override fun encrypt(plain: String) = "enc(" + plain.reversed() + ")"
            override fun decrypt(blob: String): String = if (blob == "garbage") throw UnusableBlob("bad") else blob.removePrefix("enc(").removeSuffix(")").reversed()
        }
        assertEquals("tok", TokenStore(kv2, c2).get())
        assertEquals("enc(kot)", kv2.get(TokenStore.ENC))
        assertNull(kv2.get(TokenStore.PLAIN))
        // the key store has a passing problem: nothing is deleted at all
        val kv3 = FakeKv(mutableMapOf(TokenStore.PLAIN to "tok", TokenStore.ENC to "enc(kot)"))
        assertEquals("tok", TokenStore(kv3, FakeCrypto(transientFailures = 1)).get())
        assertEquals("tok", kv3.get(TokenStore.PLAIN)); assertEquals("enc(kot)", kv3.get(TokenStore.ENC))
    }

    @Test fun savingFailsLoudlyAndKeepsWhatWasThere() {
        val kv = FakeKv(); TokenStore(kv, FakeCrypto()).set("first")
        assertThrows(IllegalStateException::class.java) { TokenStore(kv, FakeCrypto(failEncrypt = true)).set("second") }
        assertEquals("first", TokenStore(kv, FakeCrypto()).get())
    }

    @Test fun anEmptyTokenClearsEverything() {
        val kv = FakeKv(); val s = TokenStore(kv, FakeCrypto()); s.set("tok"); s.set("   ")
        assertEquals("", s.get()); assertNull(kv.get(TokenStore.ENC)); assertNull(kv.get(TokenStore.PLAIN))
    }

    @Test fun theDecryptedTokenIsCachedSoTheKeyStoreIsNotUsedForEveryTrack() {
        val kv = FakeKv(); TokenStore(kv, FakeCrypto()).set("tok")
        val crypto = FakeCrypto()
        val s = TokenStore(kv, crypto)
        repeat(5) { assertEquals("tok", s.get()) }
        assertEquals(1, crypto.decrypts)
    }

    @Test fun aSlowReadOfTheOldTokenCannotOverwriteANewSave() {
        // reader thread is inside decrypt of the OLD blob while the main thread saves a new token
        val kv = FakeKv(); TokenStore(kv, FakeCrypto()).set("old")
        val inDecrypt = CountDownLatch(1); val release = CountDownLatch(1)
        val slow = object : Crypto {
            override fun encrypt(plain: String) = "enc(" + plain.reversed() + ")"
            override fun decrypt(blob: String): String {
                inDecrypt.countDown(); release.await(5, TimeUnit.SECONDS)
                return blob.removePrefix("enc(").removeSuffix(")").reversed()
            }
        }
        val s = TokenStore(kv, slow)
        var read: String? = null
        val reader = thread { read = s.get() }
        assertTrue(inDecrypt.await(5, TimeUnit.SECONDS))
        val saver = thread { s.set("new") }      // waits for the reader (they are serialised)
        Thread.sleep(100); release.countDown()
        reader.join(5000); saver.join(5000)
        assertEquals("old", read)
        assertEquals("the save came last and wins", "new", s.get())
        assertEquals("new", TokenStore(kv, FakeCrypto()).get())
    }

    @Test fun aFailedMigrationIsTriedAgainAfterAPauseNotOnEveryRead() {
        var clock = 1_000L
        val kv = FakeKv(mutableMapOf(TokenStore.PLAIN to "tok"))
        val crypto = FakeCrypto(failEncrypt = true)
        val s = TokenStore(kv, crypto, now = { clock })
        assertEquals("tok", s.get())                       // plaintext keeps working
        assertEquals("tok", s.get())                       // inside the pause: no new attempt
        clock += TokenStore.RETRY_MS + 1
        crypto.failEncrypt = false                         // the key store is ready now
        assertEquals("tok", s.get())
        assertEquals("enc(kot)", kv.get(TokenStore.ENC))   // migrated on the retry
        assertNull(kv.get(TokenStore.PLAIN))
    }

    @Test fun aTokenEqualToTheStoredOneIsNotEncryptedAgain() {
        val kv = FakeKv(); val c = FakeCrypto(); val s = TokenStore(kv, c)
        s.set("tok")
        c.failEncrypt = true                               // a key store that would now fail
        s.setIfChanged("  tok ")                           // nothing to do, so nothing can fail
        assertEquals("tok", s.get())
        assertThrows(IllegalStateException::class.java) { s.setIfChanged("other") }
        assertEquals("tok", s.get())
    }
}
