//! At-rest encryption for local files (history, auth session) using the Windows
//! Data Protection API (DPAPI), user scope.
//!
//! HONEST LIMITATION: DPAPI user-scope encryption ties the ciphertext to the
//! current Windows user account. It protects the data against *other* Windows
//! users on the same machine and against casual file inspection / copying the
//! file to another machine. It does NOT protect against a malicious process
//! running as the *same* user — such a process can simply call
//! `CryptUnprotectData` itself (exactly as we do here). This is an accepted
//! limitation: it raises the bar for at-rest exposure without pretending to be
//! a full secrets vault.

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
            ptr::null(),      // szDataDescr
            ptr::null(),      // pOptionalEntropy
            ptr::null(),      // pvReserved
            ptr::null(),      // pPromptStruct
            0,                // dwFlags
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
            ptr::null_mut(),  // ppszDataDescr
            ptr::null(),      // pOptionalEntropy
            ptr::null(),      // pvReserved
            ptr::null(),      // pPromptStruct
            0,                // dwFlags
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
