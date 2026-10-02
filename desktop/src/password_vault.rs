//! Saved connection passwords, encrypted at rest.
//!
//! The OS keystore holds exactly **one** secret for MultiDB: a random 32-byte
//! data key. Every connection password is encrypted with it (AES-256-GCM) and
//! the ciphertext lives in `secrets.json` beside the history database. That
//! file is useless on its own — without the data key it is 32 bytes of nonce
//! and tag around noise.
//!
//! The one-key shape is what keeps the app out of the user's way. macOS asks
//! permission per *keychain item*, and "Always Allow" trusts one app for one
//! item, so a keychain item per connection meant a dialog per connection, every
//! time the app's code signature changed. One item means at most one dialog,
//! however many connections there are. Chrome ("Chrome Safe Storage"), VS Code
//! and Electron's `safeStorage` all store credentials this way for the same
//! reason.
//!
//! The dialog only stays gone if macOS recognises the app across rebuilds,
//! which means every build has to be signed with the same identity — see
//! `build/darwin/bundle.sh`. An unsigned or ad-hoc-signed bundle is a different
//! application to the keychain every time it is compiled, and no amount of
//! "Always Allow" survives that.
//!
//! Passwords saved by earlier versions are still in their own keychain items.
//! They are migrated on first read — one last dialog each — and the old items
//! are deleted as they go.

use aes_gcm::aead::{Aead, Generate, Nonce};
use aes_gcm::{Aes256Gcm, Key, KeyInit};
use anyhow::{anyhow, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Keystore entry holding the data key. One per installation, not per
/// connection — see the module comment.
const KEYSTORE_SERVICE: &str = "MultiDB Safe Storage";
const KEYSTORE_ACCOUNT: &str = "data-key";
#[cfg(windows)]
const KEYSTORE_TARGET: &str = "MultiDB.SafeStorage";

/// Where passwords were kept before the data key existed: one item each.
const LEGACY_SERVICE: &str = "multidb";
const LEGACY_ACCOUNT_PREFIX: &str = "connection:";
#[cfg(windows)]
const LEGACY_WINDOWS_PREFIX: &str = "multidb.connection.";

// ─── Public API ─────────────────────────────────────────────────────────────

pub fn save_connection_password(conn_id: &str, password: &str) -> Result<()> {
    if password.is_empty() {
        return delete_connection_password(conn_id);
    }
    let sealed = vault()?.seal(password)?;
    let mut secrets = Secrets::read(&secrets_path()?)?;
    secrets.connections.insert(conn_id.to_string(), sealed);
    secrets.write(&secrets_path()?)?;
    legacy::delete(conn_id).ok();

    match load_connection_password(conn_id)? {
        Some(saved) if saved == password => Ok(()),
        Some(_) => Err(anyhow!("saved password verification failed for connection {conn_id}")),
        None => Err(anyhow!("saved password verification found nothing for connection {conn_id}")),
    }
}

pub fn load_connection_password(conn_id: &str) -> Result<Option<String>> {
    let secrets = Secrets::read(&secrets_path()?)?;
    if let Some(sealed) = secrets.connections.get(conn_id) {
        return vault()?.open(sealed).map(Some).with_context(|| {
            format!(
                "decrypt the saved password for connection {conn_id}. The data key in the OS keystore \
                 does not match the one it was saved with; re-enter the password to store it again"
            )
        });
    }
    // Saved by an earlier version, one keychain item per connection. Move it
    // across now so this is the last time the OS asks about it.
    let Some(password) = legacy::load(conn_id)? else { return Ok(None) };
    save_connection_password(conn_id, &password)?;
    Ok(Some(password))
}

pub fn delete_connection_password(conn_id: &str) -> Result<()> {
    let path = secrets_path()?;
    let mut secrets = Secrets::read(&path)?;
    if secrets.connections.remove(conn_id).is_some() {
        secrets.write(&path)?;
    }
    legacy::delete(conn_id)
}

// ─── The data key ───────────────────────────────────────────────────────────

/// The cipher built from the data key, read from the keystore once per run.
///
/// Caching matters as much as the single-item design: it is what makes the
/// keystore touched once at startup rather than on every connect.
fn vault() -> Result<Vault> {
    static CACHED: Mutex<Option<[u8; 32]>> = Mutex::new(None);
    let mut cached = CACHED.lock().map_err(|_| anyhow!("data key lock poisoned"))?;
    if let Some(key) = *cached {
        return Ok(Vault::new(key));
    }
    let key = match keystore::load()? {
        Some(key) => key,
        None => {
            let key = Key::<Aes256Gcm>::try_generate().map_err(|err| anyhow!("generate a data key: {err}"))?;
            let key: [u8; 32] = key.into();
            keystore::store(&key)?;
            key
        }
    };
    *cached = Some(key);
    Ok(Vault::new(key))
}

struct Vault {
    cipher: Aes256Gcm,
}

impl Vault {
    fn new(key: [u8; 32]) -> Self {
        Vault { cipher: Aes256Gcm::new(&key.into()) }
    }

    /// `hex(nonce ‖ ciphertext ‖ tag)`. The nonce is fresh per password, which
    /// is what lets one key protect all of them.
    fn seal(&self, plaintext: &str) -> Result<String> {
        let nonce = Nonce::<Aes256Gcm>::try_generate().map_err(|err| anyhow!("generate a nonce: {err}"))?;
        let sealed = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| anyhow!("encrypt the password"))?;
        let mut bytes = nonce.to_vec();
        bytes.extend_from_slice(&sealed);
        Ok(hex_encode(&bytes))
    }

    fn open(&self, sealed: &str) -> Result<String> {
        let bytes = hex_decode(sealed)?;
        if bytes.len() <= 12 {
            return Err(anyhow!("stored password is too short to be valid"));
        }
        let (nonce, body) = bytes.split_at(12);
        let nonce: [u8; 12] =
            nonce.try_into().map_err(|_| anyhow!("stored password has a malformed nonce"))?;
        let plain = self
            .cipher
            .decrypt(&Nonce::<Aes256Gcm>::from(nonce), body)
            .map_err(|_| anyhow!("the stored password could not be decrypted"))?;
        String::from_utf8(plain).context("decode the stored password as UTF-8")
    }
}

// ─── The secrets file ───────────────────────────────────────────────────────

#[derive(Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Secrets {
    version: u32,
    /// Connection id → `hex(nonce ‖ ciphertext ‖ tag)`.
    #[serde(default)]
    connections: BTreeMap<String, String>,
}

impl Secrets {
    /// A missing or unreadable file reads as empty: a vault with nothing in it
    /// is the same situation as a fresh install, and refusing to start because
    /// of it would help nobody.
    fn read(path: &Path) -> Result<Self> {
        let Ok(text) = std::fs::read_to_string(path) else { return Ok(Secrets::default()) };
        Ok(serde_json::from_str(&text).unwrap_or_default())
    }

    /// Written to a neighbouring temporary file and renamed, so an interrupted
    /// write cannot leave a half-written file where the passwords used to be.
    fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let mut body = serde_json::to_string_pretty(&Secrets { version: 1, connections: self.connections.clone() })
            .context("serialise saved passwords")?;
        body.push('\n');
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, body).with_context(|| format!("write {}", temp.display()))?;
        restrict_to_owner(&temp)?;
        std::fs::rename(&temp, path).with_context(|| format!("replace {}", path.display()))?;
        restrict_to_owner(path)
    }
}

/// The file only holds ciphertext, but there is no reason for anyone else on
/// the machine to have it.
#[cfg(unix)]
fn restrict_to_owner(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restrict permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn restrict_to_owner(_path: &Path) -> Result<()> {
    Ok(())
}

fn secrets_path() -> Result<PathBuf> {
    let base = dirs::config_dir().ok_or_else(|| anyhow!("no configuration directory for this user"))?;
    Ok(base.join("multidb").join("secrets.json"))
}

// ─── Hex ────────────────────────────────────────────────────────────────────

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap_or('0'));
    }
    out
}

fn hex_decode(text: &str) -> Result<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return Err(anyhow!("stored value is not valid hex"));
    }
    let bytes: Vec<u8> = text.as_bytes().to_vec();
    bytes
        .chunks(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16);
            let low = (pair[1] as char).to_digit(16);
            match (high, low) {
                (Some(high), Some(low)) => Ok(((high << 4) | low) as u8),
                _ => Err(anyhow!("stored value is not valid hex")),
            }
        })
        .collect()
}

// ─── The keystore, per platform ─────────────────────────────────────────────

mod keystore {
    use super::{hex_decode, hex_encode};
    use anyhow::{anyhow, Context, Result};

    #[cfg(not(windows))]
    pub fn load() -> Result<Option<[u8; 32]>> {
        let entry = entry()?;
        match entry.get_password() {
            Ok(text) => parse(&text).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(err).context("read the MultiDB data key from the OS keystore"),
        }
    }

    #[cfg(not(windows))]
    pub fn store(key: &[u8; 32]) -> Result<()> {
        entry()?
            .set_password(&hex_encode(key))
            .context("save the MultiDB data key to the OS keystore")
    }

    #[cfg(not(windows))]
    fn entry() -> Result<keyring::Entry> {
        keyring::Entry::new(super::KEYSTORE_SERVICE, super::KEYSTORE_ACCOUNT)
            .context("open the MultiDB keystore entry")
    }

    #[cfg(windows)]
    pub fn load() -> Result<Option<[u8; 32]>> {
        match super::windows_credential::read(super::KEYSTORE_TARGET)? {
            Some(text) => parse(&text).map(Some),
            None => Ok(None),
        }
    }

    #[cfg(windows)]
    pub fn store(key: &[u8; 32]) -> Result<()> {
        super::windows_credential::write(super::KEYSTORE_TARGET, "MultiDB", &hex_encode(key))
    }

    fn parse(text: &str) -> Result<[u8; 32]> {
        let bytes = hex_decode(text.trim())?;
        <[u8; 32]>::try_from(bytes.as_slice())
            .map_err(|_| anyhow!("the MultiDB data key in the OS keystore is the wrong length"))
    }
}

// ─── Passwords saved by earlier versions ────────────────────────────────────

mod legacy {
    use anyhow::{Context, Result};

    fn account(conn_id: &str) -> String {
        format!("{}{conn_id}", super::LEGACY_ACCOUNT_PREFIX)
    }

    fn entry(conn_id: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(super::LEGACY_SERVICE, &account(conn_id)).context("open the old keychain entry")
    }

    pub fn load(conn_id: &str) -> Result<Option<String>> {
        #[cfg(windows)]
        if let Some(password) = super::windows_credential::read(&format!("{}{conn_id}", super::LEGACY_WINDOWS_PREFIX))? {
            return Ok(Some(password));
        }
        match entry(conn_id)?.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(keyring::Error::NoEntry) => Ok(None),
            // A keystore that cannot be read is not the same as one with
            // nothing in it, but for a *migration* it may as well be: there is
            // nothing to move, and the caller reports the missing password.
            Err(_) => Ok(None),
        }
    }

    pub fn delete(conn_id: &str) -> Result<()> {
        #[cfg(windows)]
        super::windows_credential::delete(&format!("{}{conn_id}", super::LEGACY_WINDOWS_PREFIX))?;
        match entry(conn_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(err).with_context(|| format!("remove the old keychain item for connection {conn_id}")),
        }
    }
}

#[cfg(windows)]
mod windows_credential {
    use anyhow::{anyhow, Result};

    pub fn write(target: &str, user: &str, secret: &str) -> Result<()> {
        use std::{mem, ptr};
        use windows_sys::Win32::Security::Credentials::{
            CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
        };

        let mut target_w = wide_null(target);
        let mut user_w = wide_null(user);
        let mut blob = secret.as_bytes().to_vec();
        let credential = CREDENTIALW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: target_w.as_mut_ptr(),
            Comment: ptr::null_mut(),
            LastWritten: unsafe { mem::zeroed() },
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: ptr::null_mut(),
            TargetAlias: ptr::null_mut(),
            UserName: user_w.as_mut_ptr(),
        };
        if unsafe { CredWriteW(&credential, 0) } == 0 {
            return Err(anyhow!("save {target} to Windows Credential Manager: {}", last_error()));
        }
        Ok(())
    }

    pub fn read(target: &str) -> Result<Option<String>> {
        use std::{ptr, slice};
        use windows_sys::Win32::{
            Foundation::{GetLastError, ERROR_NOT_FOUND},
            Security::Credentials::{CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC},
        };

        let target_w = wide_null(target);
        let mut credential_ptr: *mut CREDENTIALW = ptr::null_mut();
        if unsafe { CredReadW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential_ptr) } == 0 {
            let code = unsafe { GetLastError() };
            if code == ERROR_NOT_FOUND {
                return Ok(None);
            }
            return Err(anyhow!("read {target} from Windows Credential Manager: error {code}"));
        }
        let secret = unsafe {
            let credential = &*credential_ptr;
            let blob = slice::from_raw_parts(credential.CredentialBlob, credential.CredentialBlobSize as usize);
            let secret = String::from_utf8(blob.to_vec());
            CredFree(credential_ptr.cast());
            secret
        }
        .map_err(|_| anyhow!("decode {target} from Windows Credential Manager as UTF-8"))?;
        Ok(Some(secret))
    }

    pub fn delete(target: &str) -> Result<()> {
        use windows_sys::Win32::{
            Foundation::{GetLastError, ERROR_NOT_FOUND},
            Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC},
        };

        let target_w = wide_null(target);
        if unsafe { CredDeleteW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
            let code = unsafe { GetLastError() };
            if code != ERROR_NOT_FOUND {
                return Err(anyhow!("remove {target} from Windows Credential Manager: error {code}"));
            }
        }
        Ok(())
    }

    fn wide_null(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn last_error() -> String {
        format!("error {}", unsafe { windows_sys::Win32::Foundation::GetLastError() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault_with_known_key() -> Vault {
        Vault::new([7u8; 32])
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("multidb-vault-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn a_password_survives_a_round_trip() {
        let vault = vault_with_known_key();
        for password in ["hunter2", "", "pässwörd with spaces", "🔐"] {
            let sealed = vault.seal(password).expect("seal");
            assert_eq!(vault.open(&sealed).expect("open"), password);
            // Nothing recognisable is left in the stored form.
            assert!(!sealed.contains("hunter"), "{sealed}");
        }
    }

    #[test]
    fn the_same_password_never_seals_to_the_same_bytes() {
        // A fresh nonce each time, so identical passwords on two connections
        // are not visibly identical in the file.
        let vault = vault_with_known_key();
        assert_ne!(vault.seal("same").expect("seal"), vault.seal("same").expect("seal"));
    }

    #[test]
    fn a_tampered_or_foreign_secret_is_refused_rather_than_guessed_at() {
        let vault = vault_with_known_key();
        let sealed = vault.seal("hunter2").expect("seal");

        // One flipped character fails the authentication tag.
        let mut tampered: Vec<char> = sealed.chars().collect();
        let last = tampered.len() - 1;
        tampered[last] = if tampered[last] == 'a' { 'b' } else { 'a' };
        assert!(vault.open(&tampered.iter().collect::<String>()).is_err());

        // A different data key — the case where the keystore entry was lost and
        // regenerated — must not silently return rubbish.
        assert!(Vault::new([9u8; 32]).open(&sealed).is_err());

        // And nonsense in the file is an error, not a panic.
        assert!(vault.open("").is_err());
        assert!(vault.open("nothex").is_err());
        assert!(vault.open("abc").is_err());
    }

    #[test]
    fn the_secrets_file_round_trips_and_starts_empty() {
        let dir = tempdir();
        let path = dir.join("secrets.json");
        assert_eq!(Secrets::read(&path).expect("read missing"), Secrets::default());

        let mut secrets = Secrets::default();
        secrets.connections.insert("conn-1".into(), "deadbeef".into());
        secrets.write(&path).expect("write");
        let read = Secrets::read(&path).expect("read");
        assert_eq!(read.connections.get("conn-1").map(String::as_str), Some("deadbeef"));
        assert_eq!(read.version, 1);

        // A corrupted file reads as empty rather than stopping the app.
        std::fs::write(&path, "{ not json").expect("write junk");
        assert_eq!(Secrets::read(&path).expect("read junk").connections.len(), 0);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            secrets.write(&path).expect("rewrite");
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "the file should only be readable by its owner");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hex_round_trips_and_rejects_rubbish() {
        assert_eq!(hex_encode(&[0x00, 0x0f, 0xff]), "000fff");
        assert_eq!(hex_decode("000fff").expect("decode"), vec![0x00, 0x0f, 0xff]);
        assert!(hex_decode("abc").is_err());
        assert!(hex_decode("zz").is_err());
    }
}
