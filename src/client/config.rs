use serde::{Deserialize, Serialize};
use std::{
    env, fs, io,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct Server {
    pub name: String,
    pub address: String,
}

pub(super) fn path() -> io::Result<PathBuf> {
    #[cfg(windows)]
    let base = env::var_os("APPDATA")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no configuration directory (set APPDATA)",
            )
        })?;
    #[cfg(not(windows))]
    let base = env::var_os("XDG_CONFIG_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|p| !p.is_empty())
                .map(|p| PathBuf::from(p).join(".config"))
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no configuration directory (set XDG_CONFIG_HOME or HOME)",
            )
        })?;
    Ok(base.join("tpokr").join("servers.json"))
}

pub(super) fn session(profile: &str) -> io::Result<String> {
    if profile.is_empty()
        || profile.len() > 64
        || !profile
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "profile must contain only letters, numbers, '-' or '_'",
        ));
    }
    let path = path()?.with_file_name(format!("session-{profile}"));
    session_at(&path)
}

fn session_at(path: &Path) -> io::Result<String> {
    fs::create_dir_all(path.parent().unwrap())?;
    let token = super::new_session();
    match create_private_file(path) {
        Ok(mut file) => {
            file.write_all(token.as_bytes())?;
            Ok(token)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let existing = fs::read_to_string(path)?;
            if existing.len() == 64 && existing.bytes().all(|b| b.is_ascii_hexdigit()) {
                Ok(existing)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid session file",
                ))
            }
        }
        Err(error) => Err(error),
    }
}

#[cfg(not(windows))]
fn create_private_file(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(windows)]
fn create_private_file(path: &Path) -> io::Result<fs::File> {
    use std::{
        os::windows::{ffi::OsStrExt, io::FromRawHandle},
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{INVALID_HANDLE_VALUE, LocalFree},
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE},
    };
    // Protected DACL grants access to the file owner and SYSTEM, with no inherited entries.
    let sddl: Vec<u16> = "D:P(A;;FA;;;OW)(A;;FA;;;SY)\0".encode_utf16().collect();
    let mut descriptor = ptr::null_mut();
    // SAFETY: the UTF-16 string is null-terminated and descriptor receives an allocated pointer.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: all pointers are valid for the call, and the security descriptor remains allocated.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_GENERIC_WRITE,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    let error = (handle == INVALID_HANDLE_VALUE).then(io::Error::last_os_error);
    // SAFETY: the descriptor was allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
    unsafe {
        LocalFree(descriptor);
    }
    match error {
        Some(error) => Err(error),
        // SAFETY: CreateFileW returned a valid owned file handle.
        None => Ok(unsafe { fs::File::from_raw_handle(handle) }),
    }
}

pub(super) fn load() -> io::Result<Vec<Server>> {
    match fs::read(path()?) {
        Ok(data) => serde_json::from_slice(&data).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(vec![Server {
            name: "Local server".into(),
            address: "127.0.0.1:7878".into(),
        }]),
        Err(error) => Err(error),
    }
}

pub(super) fn save(servers: &[Server]) -> io::Result<()> {
    save_at(&path()?, servers)
}

fn save_at(path: &Path, servers: &[Server]) -> io::Result<()> {
    fs::create_dir_all(path.parent().unwrap())?;
    let data = serde_json::to_vec_pretty(servers)?;
    // Replace the file only after writing the complete configuration.
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temporary, data)?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(temporary);
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_can_be_replaced_and_session_reused() {
        let dir =
            env::temp_dir().join(format!("tpokr-config-test-{}", super::super::new_session()));
        let config = dir.join("servers.json");
        let servers = [Server {
            name: "one".into(),
            address: "localhost:7878".into(),
        }];
        save_at(&config, &servers).unwrap();
        let updated = [Server {
            name: "two".into(),
            address: "localhost:7879".into(),
        }];
        save_at(&config, &updated).unwrap();
        assert_eq!(
            serde_json::from_slice::<Vec<Server>>(&fs::read(&config).unwrap()).unwrap(),
            updated
        );
        let session = dir.join("session-test");
        let token = session_at(&session).unwrap();
        assert_eq!(session_at(&session).unwrap(), token);
        assert_eq!(token.len(), 64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&session).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
