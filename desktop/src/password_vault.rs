use anyhow::anyhow;
use anyhow::{Context, Result};

const KEYCHAIN_SERVICE: &str = "multidb";
const LEGACY_ACCOUNT_PREFIX: &str = "connection:";

#[cfg(windows)]
const WINDOWS_TARGET_PREFIX: &str = "multidb.connection.";

fn legacy_connection_account(conn_id: &str) -> String {
    format!("{LEGACY_ACCOUNT_PREFIX}{conn_id}")
}

fn legacy_connection_entry(conn_id: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYCHAIN_SERVICE, &legacy_connection_account(conn_id))
        .context("create legacy keychain entry")
}

#[cfg(not(windows))]
pub fn save_connection_password(conn_id: &str, password: &str) -> Result<()> {
    let entry = legacy_connection_entry(conn_id)?;
    if password.is_empty() {
        delete_connection_password(conn_id)?;
        return Ok(());
    }
    entry
        .set_password(password)
        .with_context(|| format!("save password to OS keychain for connection {conn_id}"))?;

    match load_connection_password(conn_id)? {
        Some(saved) if saved == password => Ok(()),
        Some(_) => Err(anyhow!(
            "OS keychain verification failed for connection {conn_id}"
        )),
        None => Err(anyhow!(
            "OS keychain verification found no credential for connection {conn_id}"
        )),
    }
}

#[cfg(not(windows))]
pub fn load_connection_password(conn_id: &str) -> Result<Option<String>> {
    let entry = legacy_connection_entry(conn_id)?;
    match entry.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(err)
            .with_context(|| format!("load password from OS keychain for connection {conn_id}")),
    }
}

#[cfg(not(windows))]
pub fn delete_connection_password(conn_id: &str) -> Result<()> {
    let entry = legacy_connection_entry(conn_id)?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err)
            .with_context(|| format!("delete password from OS keychain for connection {conn_id}")),
    }
}

#[cfg(windows)]
pub fn save_connection_password(conn_id: &str, password: &str) -> Result<()> {
    if password.is_empty() {
        delete_connection_password(conn_id)?;
        return Ok(());
    }

    write_windows_credential(conn_id, password)?;

    match load_connection_password(conn_id)? {
        Some(saved) if saved == password => {
            let _ = delete_legacy_keyring_password(conn_id);
            Ok(())
        }
        Some(_) => Err(anyhow!(
            "OS keychain verification failed for connection {conn_id}"
        )),
        None => Err(anyhow!(
            "OS keychain verification found no credential for connection {conn_id}"
        )),
    }
}

#[cfg(windows)]
pub fn load_connection_password(conn_id: &str) -> Result<Option<String>> {
    if let Some(password) = read_windows_credential(conn_id)? {
        return Ok(Some(password));
    }

    let legacy = load_legacy_keyring_password(conn_id)?;
    if let Some(ref password) = legacy {
        write_windows_credential(conn_id, password)?;
        let _ = delete_legacy_keyring_password(conn_id);
    }
    Ok(legacy)
}

#[cfg(windows)]
pub fn delete_connection_password(conn_id: &str) -> Result<()> {
    delete_windows_credential(conn_id)?;
    let _ = delete_legacy_keyring_password(conn_id);
    Ok(())
}

#[cfg(windows)]
fn windows_target_name(conn_id: &str) -> String {
    format!("{WINDOWS_TARGET_PREFIX}{conn_id}")
}

#[cfg(windows)]
fn load_legacy_keyring_password(conn_id: &str) -> Result<Option<String>> {
    let entry = legacy_connection_entry(conn_id)?;
    match entry.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(err)
            .with_context(|| format!("load legacy keychain password for connection {conn_id}")),
    }
}

#[cfg(windows)]
fn delete_legacy_keyring_password(conn_id: &str) -> Result<()> {
    let entry = legacy_connection_entry(conn_id)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err)
            .with_context(|| format!("delete legacy keychain password for connection {conn_id}")),
    }
}

#[cfg(windows)]
fn write_windows_credential(conn_id: &str, password: &str) -> Result<()> {
    use std::{mem, ptr};
    use windows_sys::Win32::Security::Credentials::{
        CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    let target = windows_target_name(conn_id);
    let mut target_w = wide_null(&target);
    let mut user_w = wide_null(conn_id);
    let mut blob = password.as_bytes().to_vec();

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

    let ok = unsafe { CredWriteW(&credential, 0) };
    if ok == 0 {
        return Err(anyhow!(last_windows_error_message())).with_context(|| {
            format!("save password to Windows Credential Manager for connection {conn_id}")
        });
    }
    Ok(())
}

#[cfg(windows)]
fn read_windows_credential(conn_id: &str) -> Result<Option<String>> {
    use std::{ptr, slice};
    use windows_sys::Win32::{
        Foundation::{GetLastError, ERROR_NOT_FOUND},
        Security::Credentials::{CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC},
    };

    let target = windows_target_name(conn_id);
    let target_w = wide_null(&target);
    let mut credential_ptr: *mut CREDENTIALW = ptr::null_mut();

    let ok = unsafe { CredReadW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential_ptr) };
    if ok == 0 {
        let code = unsafe { GetLastError() };
        if code == ERROR_NOT_FOUND {
            return Ok(None);
        }
        return Err(anyhow!(format!("Windows error code {code}"))).with_context(|| {
            format!("load password from Windows Credential Manager for connection {conn_id}")
        });
    }

    let password = unsafe {
        let credential = &*credential_ptr;
        let blob = slice::from_raw_parts(
            credential.CredentialBlob,
            credential.CredentialBlobSize as usize,
        );
        let password = String::from_utf8(blob.to_vec())
            .context("decode Windows Credential Manager secret as UTF-8")?;
        CredFree(credential_ptr.cast());
        password
    };

    Ok(Some(password))
}

#[cfg(windows)]
fn delete_windows_credential(conn_id: &str) -> Result<()> {
    use windows_sys::Win32::{
        Foundation::{GetLastError, ERROR_NOT_FOUND},
        Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC},
    };

    let target = windows_target_name(conn_id);
    let target_w = wide_null(&target);
    let ok = unsafe { CredDeleteW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0) };
    if ok == 0 {
        let code = unsafe { GetLastError() };
        if code == ERROR_NOT_FOUND {
            return Ok(());
        }
        return Err(anyhow!(format!("Windows error code {code}"))).with_context(|| {
            format!("delete password from Windows Credential Manager for connection {conn_id}")
        });
    }
    Ok(())
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn last_windows_error_message() -> String {
    let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
    format!("Windows error code {code}")
}

#[cfg(test)]
mod tests {
    use super::legacy_connection_account;

    #[test]
    fn legacy_connection_account_uses_stable_prefix() {
        assert_eq!(legacy_connection_account("abc-123"), "connection:abc-123");
    }

    #[cfg(windows)]
    #[test]
    fn windows_target_name_is_stable() {
        assert_eq!(
            super::windows_target_name("abc-123"),
            "multidb.connection.abc-123"
        );
    }
}
