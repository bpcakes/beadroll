//! Machine identity and aliases, plus random event ids and hexadecimal tokens.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use crate::layout;
use beadroll_core::json::{self, Format, Object, Value};
use beadroll_git::IoError;

use crate::error::Error;
use crate::fs::{FileMode, fsync, json_text, private_dir};
use crate::settings::{Clock, Settings};

/// A UUIDv7 event id: clock milliseconds followed by random bits. Clock order is for display;
/// deps determine causality.
pub fn uuid7(clock: &Clock) -> String {
    let millis = (clock.now_ns().div_euclid(1_000_000) as u128) & ((1 << 48) - 1);
    let random = uuid::Uuid::new_v4().as_u128();
    let value = millis << 80
        | 0x7 << 76
        | ((random >> 64) & 0xfff) << 64
        | 0b10 << 62
        | (random & ((1 << 62) - 1));
    uuid::Uuid::from_u128(value).hyphenated().to_string()
}

/// Exactly `n` random lowercase hexadecimal digits.
pub fn hex_token(n: usize) -> String {
    let mut out = String::new();
    while out.len() < n {
        out.push_str(&uuid::Uuid::new_v4().simple().to_string());
    }
    out.truncate(n);
    out
}

/// The opaque id of this machine, from `$BEAD_HOME/replica.json`, if it has one yet: a read never
/// mints one ([`ensure_replica_id`] does). The hostname stays in a local alias, never in events.
pub fn replica_id(settings: &Settings) -> Result<Option<String>, Error> {
    let path = settings.home()?.join(layout::REPLICA);
    if !path.exists() {
        return Ok(None);
    }
    read_replica_id(&path).map(Some)
}

/// The opaque id of this machine, minted first if it has none: what every event it writes
/// carries. The file is written whole under a temporary name and then linked into place, so it is
/// never seen half done, and the first writer wins.
pub fn ensure_replica_id(settings: &Settings) -> Result<String, Error> {
    let home = settings.home()?;
    let path = home.join(layout::REPLICA);
    if !path.exists() {
        private_dir(&home)?;
        mint(&path, settings)?;
    }
    read_replica_id(&path)
}

/// The id `path`, a machine identity file, holds: eight lowercase hex digits, or an error that says
/// to fix the file rather than replace it.
fn read_replica_id(path: &Path) -> Result<String, Error> {
    let raw = std::fs::read(path).map_err(|error| {
        Error::io_message(format!(
            "{}: {} (fix its access; do not replace it: the claims this machine holds belong to \
             that identity)",
            beadroll_git::strerror(&error),
            path.display()
        ))
    })?;
    let ident = json_text(&raw)
        .ok()
        .and_then(|value| match value.get("id") {
            Some(Value::Str(id))
                if id.len() == 8 && beadroll_core::text::is_lower_hex(id.as_bytes()) =>
            {
                Some(id.clone())
            }
            _ => None,
        });
    ident.ok_or_else(|| {
        Error::Identity(format!(
            "{} is not a valid machine identity; restore it, or move it aside to mint a new one \
             (claims held under the old identity then need `bead takeover`)",
            path.display()
        ))
    })
}

/// Writes a new identity beside `path` and links it into place; another process may win.
fn mint(path: &Path, settings: &Settings) -> Result<(), Error> {
    let tmp = path.with_file_name(format!(".replica.{}.tmp", std::process::id()));
    let mut body = Object::new();
    body.insert("id".into(), Value::from(hex_token(8)));
    body.insert("alias".into(), Value::from(short_hostname()));
    let text = json::write(&Value::from(body), Format::ASCII)?;
    let written = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(FileMode::Private.bits())
        .open(&tmp)
        .map_err(|e| IoError::at(&e, &tmp))
        .and_then(|mut file| {
            file.write_all(text.as_bytes())
                .map_err(|e| IoError::at(&e, path))?;
            fsync(&file, path)
        });
    if let Err(error) = written {
        // nothing was linked: what was written of the temporary file goes with the attempt
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::Io(error));
    }
    settings.failpoints.hit("identity.before-link");
    let linked = match std::fs::hard_link(&tmp, path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()), // another process won
        Err(error) => Err(IoError::at(&error, path)),
    };
    // Always remove the temporary file after linking; a cleanup failure takes precedence.
    crate::journal::remove(&tmp)?;
    linked.map_err(Error::Io)
}

/// The hostname up to its first dot.
fn short_hostname() -> String {
    let uname = rustix::system::uname();
    let name = uname.nodename().to_string_lossy();
    name.split('.').next().unwrap_or_default().to_owned()
}

/// Local machine display names, from this machine's replica.json alias then aliases.json. Skip
/// unreadable or malformed files and entries.
pub fn machine_aliases(settings: &Settings) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(home) = settings.home() else {
        return out;
    };
    let read = |name: &str| crate::fs::read_json_text(&home.join(name));
    if let Ok(Value::Object(own)) = read(layout::REPLICA)
        && let Some(Value::Str(id)) = own.get("id")
    {
        let alias = match own.get("alias") {
            Some(Value::Str(alias)) => alias.clone(),
            _ => id.clone(),
        };
        out.insert(id.clone(), alias);
    }
    if let Ok(Value::Object(aliases)) = read(layout::ALIASES) {
        for (id, alias) in *aliases {
            if let Value::Str(alias) = alias {
                out.insert(id, alias);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_ids_are_uuid7_with_the_clock_in_front() {
        let id = uuid7(&Clock::shifted(0));
        let parsed = beadroll_core::id::EventId::parse(&id).unwrap();
        assert_eq!(&id[14..15], "7");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
        let now = Clock::system().now_ns() / 1_000_000;
        assert!((now - i128::from(parsed.millis())).abs() < 5_000);
        assert_eq!(hex_token(8).len(), 8);
        assert_eq!(hex_token(40).len(), 40);
    }

    /// AS02: an identity that cannot be written whole (here the device is full) leaves nothing
    /// beside the one it was for, neither the identity nor its temporary file.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_failed_identity_write_leaves_no_temporary_file() {
        use std::ffi::OsString;

        let home = tempfile::tempdir().unwrap();
        let mut vars = std::collections::BTreeMap::new();
        vars.insert(OsString::from("BEAD_HOME"), OsString::from(home.path()));
        let settings =
            Settings::from_process(vars, None, OsString::new(), home.path().to_owned(), &|_| {
                None
            })
            .unwrap();
        let path = home.path().join(layout::REPLICA);
        let tmp = home
            .path()
            .join(format!(".replica.{}.tmp", std::process::id()));
        // every write to /dev/full fails with ENOSPC, and the open follows the link
        std::os::unix::fs::symlink("/dev/full", &tmp).unwrap();
        let failed = mint(&path, &settings).unwrap_err();
        assert_eq!(failed.code(), beadroll_core::code::Code::Io, "{failed}");
        let left: Vec<_> = std::fs::read_dir(home.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, Vec::<OsString>::new());
    }
}
