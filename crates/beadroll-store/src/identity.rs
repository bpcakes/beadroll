//! This machine's identity (Python's `replica_id` and `machine_aliases`) and the random names bead
//! mints: event ids (`uuid7`) and hex tokens.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use beadroll_core::json::{self, Format, Object, Value};
use beadroll_git::IoError;

use crate::error::Error;
use crate::fs::{fsync, json_text, private_dir};
use crate::settings::{Clock, Settings};

/// A time-ordered event id (UUIDv7, Python's `uuid7`): the clock's milliseconds, then random bits.
/// The clock part orders displays only; causality comes from `deps`.
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

/// `n` random lowercase hex digits (Python's `uuid.uuid4().hex[:n]`, `secrets.token_hex(n / 2)`).
pub fn hex_token(n: usize) -> String {
    let mut out = String::new();
    while out.len() < n {
        out.push_str(&uuid::Uuid::new_v4().simple().to_string());
    }
    out.truncate(n);
    out
}

/// The opaque id of this machine, from `$BEAD_HOME/replica.json`; with `create`, minted on first
/// use. The hostname stays in a local alias, never in events. The file is written whole under a
/// temporary name and then linked into place, so it is never seen half done, and the first writer
/// wins.
pub fn replica_id(settings: &Settings, create: bool) -> Result<Option<String>, Error> {
    let home = settings.home()?;
    let path = home.join("replica.json");
    if !path.exists() {
        if !create {
            return Ok(None);
        }
        private_dir(&home)?;
        mint(&path)?;
    }
    let raw = std::fs::read(&path).map_err(|error| {
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
            Some(Value::Str(id)) if id.len() == 8 && id.bytes().all(is_lower_hex) => {
                Some(id.clone())
            }
            _ => None,
        });
    ident.map(Some).ok_or_else(|| {
        Error::Identity(format!(
            "{} is not a valid machine identity; restore it, or move it aside to mint a new one \
             (claims held under the old identity then need `bead takeover`)",
            path.display()
        ))
    })
}

fn is_lower_hex(b: u8) -> bool {
    b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
}

/// Writes a new identity beside `path` and links it into place; another process may win.
fn mint(path: &Path) -> Result<(), Error> {
    let tmp = path.with_file_name(format!(".replica.{}.tmp", std::process::id()));
    let mut body = Object::new();
    body.insert("id".into(), Value::from(hex_token(8)));
    body.insert("alias".into(), Value::from(short_hostname()));
    let text = json::write(&Value::Object(body), Format::ASCII)
        .map_err(|e| Error::io_message(e.to_string()))?;
    let written = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| IoError::at(&e, &tmp))
        .and_then(|mut file| {
            file.write_all(text.as_bytes())
                .map_err(|e| IoError::bare(&e))?;
            fsync(&file)
        });
    let linked = written.and_then(|()| match std::fs::hard_link(&tmp, path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()), // another process won
        Err(error) => Err(IoError::at(&error, path)),
    });
    let _ = std::fs::remove_file(&tmp);
    linked.map_err(Error::Io)
}

/// The host's name up to its first dot (Python's `socket.gethostname().split(".")[0]`).
fn short_hostname() -> String {
    let uname = rustix::system::uname();
    let name = uname.nodename().to_string_lossy();
    name.split('.').next().unwrap_or_default().to_owned()
}

/// Local display names of machines: this machine's own alias from `replica.json`, then
/// `aliases.json`. Anything unreadable or malformed is skipped.
pub fn machine_aliases(settings: &Settings) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(home) = settings.home() else {
        return out;
    };
    if let Ok(Value::Object(own)) = crate::fs::read_json_text(&home.join("replica.json"))
        && let Some(Value::Str(id)) = own.get("id")
    {
        let alias = match own.get("alias") {
            Some(Value::Str(alias)) => alias.clone(),
            _ => id.clone(),
        };
        out.insert(id.clone(), alias);
    }
    if let Ok(Value::Object(aliases)) = crate::fs::read_json_text(&home.join("aliases.json")) {
        for (id, alias) in aliases {
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
}
