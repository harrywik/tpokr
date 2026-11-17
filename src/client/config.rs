use serde::{Deserialize, Serialize};
use std::{env, fs, io, io::Write, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct Server {
    pub name: String,
    pub address: String,
}

pub(super) fn path() -> io::Result<PathBuf> {
    let base = env::var_os("XDG_CONFIG_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
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
    fs::create_dir_all(path.parent().unwrap())?;
    let token = super::new_session();
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
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
    let path = path()?;
    fs::create_dir_all(path.parent().unwrap())?;
    let data = serde_json::to_vec_pretty(servers)?;
    // Replace the file only after writing the complete configuration.
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temporary, data)?;
    if let Err(error) = fs::rename(&temporary, &path) {
        let _ = fs::remove_file(temporary);
        return Err(error);
    }
    Ok(())
}
