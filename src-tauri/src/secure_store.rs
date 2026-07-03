//! At-rest encryption for local files (history, auth session).
//!
//! Platform implementations, all exposing the same `encrypt`/`decrypt` pair:
//!   - Windows: DPAPI (current-user scope).
//!   - macOS:   AES-256-GCM with a per-install random key kept in the login Keychain.
//!   - Other:   passthrough (no encryption) — for dev/CI only.
//!
//! HONEST LIMITATION: user-scope at-rest encryption ties the ciphertext (or its
//! key, on macOS) to the current OS user account. It protects the data against
//! *other* users on the same machine and against casual file inspection / copying
//! the file to another machine. It does NOT protect against a malicious process
//! running as the *same* user — such a process can obtain the key or call the
//! decrypt primitive itself (exactly as we do here). This raises the bar for
//! at-rest exposure without pretending to be a full secrets vault.
//!
//! `encrypt(plain) -> Result<cipher>`; `decrypt(cipher) -> Option<plain>` where
//! `None` means "not our ciphertext / corrupt / key unavailable". Callers treat
//! `None` as a signal to fall back to legacy plaintext parsing, so the returned
//! bytes must never be silently wrong.

pub use imp::{decrypt, encrypt};

#[cfg(target_os = "windows")]
mod imp {
    use std::ptr;

    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
    };

    /// Encrypt `plain` with DPAPI (current-user scope, no extra entropy).
    pub fn encrypt(plain: &[u8]) -> anyhow::Result<Vec<u8>> {
        // The input BLOB must point at the plaintext buffer. DPAPI does not mutate
        // it, but the API signature is not const, so we hand it a mutable pointer.
        let mut in_blob = CRYPT_INTEGER_BLOB {
            cbData: plain.len() as u32,
            pbData: plain.as_ptr() as *mut u8,
        };
        let mut out_blob = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: ptr::null_mut(),
        };

        let ok = unsafe {
            CryptProtectData(
                &mut in_blob,
                ptr::null(), // szDataDescr
                ptr::null(), // pOptionalEntropy
                ptr::null(), // pvReserved
                ptr::null(), // pPromptStruct
                0,           // dwFlags
                &mut out_blob,
            )
        };

        if ok == 0 {
            return Err(anyhow::anyhow!("CryptProtectData failed"));
        }

        // Copy the freshly-allocated output buffer into an owned Vec, then free the
        // DPAPI-allocated buffer with LocalFree.
        let cipher = unsafe {
            std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec()
        };
        unsafe {
            LocalFree(out_blob.pbData as _);
        }

        Ok(cipher)
    }

    /// Decrypt `cipher` produced by [`encrypt`]. Returns `None` on any failure
    /// (not DPAPI ciphertext, wrong user, corruption, etc.).
    pub fn decrypt(cipher: &[u8]) -> Option<Vec<u8>> {
        let mut in_blob = CRYPT_INTEGER_BLOB {
            cbData: cipher.len() as u32,
            pbData: cipher.as_ptr() as *mut u8,
        };
        let mut out_blob = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: ptr::null_mut(),
        };

        let ok = unsafe {
            CryptUnprotectData(
                &mut in_blob,
                ptr::null_mut(), // ppszDataDescr
                ptr::null(),     // pOptionalEntropy
                ptr::null(),     // pvReserved
                ptr::null(),     // pPromptStruct
                0,               // dwFlags
                &mut out_blob,
            )
        };

        if ok == 0 {
            return None;
        }

        let plain = unsafe {
            std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec()
        };
        unsafe {
            LocalFree(out_blob.pbData as _);
        }

        Some(plain)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    //! AES-256-GCM. The 32-byte key lives in the login Keychain (created lazily on
    //! first use). Ciphertext layout on disk is `[12-byte nonce][ciphertext+tag]`.

    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
    use keyring::Entry;

    const SERVICE: &str = "com.joshuatan.rewrite";
    const KEY_ACCOUNT: &str = "secure_store_master_key";
    const NONCE_LEN: usize = 12;
    const KEY_LEN: usize = 32;

    /// Fetch the master key from the Keychain, generating and persisting a fresh
    /// random one the first time.
    fn load_or_create_key() -> anyhow::Result<[u8; KEY_LEN]> {
        let entry = Entry::new(SERVICE, KEY_ACCOUNT)?;
        match entry.get_secret() {
            Ok(bytes) => {
                let arr: [u8; KEY_LEN] = bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("keychain key has unexpected length"))?;
                Ok(arr)
            }
            Err(keyring::Error::NoEntry) => {
                let key: [u8; KEY_LEN] = rand::random();
                entry.set_secret(&key)?;
                Ok(key)
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn encrypt(plain: &[u8]) -> anyhow::Result<Vec<u8>> {
        let key = load_or_create_key()?;
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));

        let nonce_bytes: [u8; NONCE_LEN] = rand::random();
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ct = cipher
            .encrypt(nonce, plain)
            .map_err(|_| anyhow::anyhow!("aes-gcm encrypt failed"))?;

        let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    pub fn decrypt(cipher_in: &[u8]) -> Option<Vec<u8>> {
        if cipher_in.len() < NONCE_LEN {
            return None;
        }
        let key = load_or_create_key().ok()?;
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));

        let (nonce_bytes, ct) = cipher_in.split_at(NONCE_LEN);
        let nonce = Nonce::from_slice(nonce_bytes);
        cipher.decrypt(nonce, ct).ok()
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod imp {
    //! Passthrough for platforms without a real backend (dev/CI on Linux). No
    //! encryption is applied; the bytes round-trip unchanged.

    pub fn encrypt(plain: &[u8]) -> anyhow::Result<Vec<u8>> {
        Ok(plain.to_vec())
    }

    pub fn decrypt(cipher: &[u8]) -> Option<Vec<u8>> {
        Some(cipher.to_vec())
    }
}
