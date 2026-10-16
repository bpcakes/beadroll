//! What every command needs: where things are, how to fail, and JSON strings.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Why a command did not succeed.
#[derive(Debug)]
pub(crate) enum Failure {
    /// The command line was wrong: exit status 2, with the usage.
    Usage(String),
    /// The command could not do its work, or what it checked failed: exit status 1.
    Failed(String),
}

impl Failure {
    pub(crate) fn io(what: &str, path: &Path, error: &std::io::Error) -> Failure {
        Failure::Failed(format!("{what} {}: {error}", path.display()))
    }
}

/// The repository's root: the directory above this crate.
pub(crate) fn root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap_or(manifest).to_path_buf()
}

/// The directory of the profile xtask was built in (`target/debug` for `cargo xtask`), where the
/// `bead` it builds lands too, whatever the target directory is called.
pub(crate) fn profile_dir() -> Result<PathBuf, Failure> {
    let exe = std::env::current_exe()
        .map_err(|error| Failure::Failed(format!("cannot find xtask's own executable: {error}")))?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| Failure::Failed(format!("{} has no directory", exe.display())))
}

/// The target directory, above the profile's.
pub(crate) fn target_dir() -> Result<PathBuf, Failure> {
    let profile = profile_dir()?;
    Ok(profile.parent().unwrap_or(&profile).to_path_buf())
}

/// A directory in the target directory for what a command writes: logs and scratch homes.
pub(crate) fn output_dir(name: &str) -> Result<PathBuf, Failure> {
    let dir = target_dir()?.join(name);
    std::fs::create_dir_all(&dir).map_err(|error| Failure::io("cannot create", &dir, &error))?;
    Ok(dir)
}

/// The Python that runs the `infra/` drivers and their tests, the first `python3` on `PATH`.
pub(crate) fn python() -> OsString {
    OsString::from("python3")
}

/// The cargo that runs xtask, for the commands that build.
pub(crate) fn cargo() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"))
}

/// The string value of the first `key` in `line`, a compact JSON object such as cargo prints for
/// each message; `None` if there is none, or its value is not a string. The pattern `"key":"`
/// matches only the object's own structure: inside a string value, quotes are escaped.
pub(crate) fn json_field(line: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{key}\":\"");
    let start = line.find(&pattern)? + pattern.len();
    let mut chars = line.get(start..)?.chars();
    let mut value = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(value),
            '\\' => value.push(match chars.next()? {
                '"' => '"',
                '\\' => '\\',
                '/' => '/',
                'b' => '\u{8}',
                'f' => '\u{c}',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                'u' => {
                    let digits: String = chars.by_ref().take(4).collect();
                    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
                        return None;
                    }
                    let unit = u32::from_str_radix(&digits, 16).ok()?;
                    char::from_u32(unit).unwrap_or(char::REPLACEMENT_CHARACTER)
                }
                _ => return None,
            }),
            c => value.push(c),
        }
    }
    None
}

/// `text` as a JSON string: quotes, backslashes and control characters escaped, the rest as is.
pub(crate) fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < ' ' || c == '\u{7f}' => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A fake tool for a test: `body` as the executable shell script `name` in `dir`, whose path this
/// is. Under `cargo test`, whose tests are threads of one process, a process another test forks
/// while the file is still open for writing holds it until it execs, and running the file
/// meanwhile fails with ETXTBSY: it is run once, with no arguments (which the bodies let pass),
/// until it starts, since later runs cannot meet that. (nextest runs a process for each test, and
/// does not meet it.)
#[cfg(test)]
pub(crate) fn fake_tool(dir: &Path, name: &str, body: &str) -> OsString {
    use std::os::unix::fs::PermissionsExt as _;

    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}")).expect("the fake tool");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("its mode");
    for _ in 0..400 {
        match std::process::Command::new(&path).output() {
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            _ => break,
        }
    }
    path.into_os_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_strings_escape_what_json_requires() {
        assert_eq!(json_string("plain"), r#""plain""#);
        assert_eq!(json_string("a\"b\\c"), r#""a\"b\\c""#);
        assert_eq!(
            json_string("\n\t\r\u{1}\u{7f}é"),
            "\"\\n\\t\\r\\u0001\\u007fé\""
        );
    }

    #[test]
    fn a_field_is_read_from_the_objects_structure_only() {
        // An escaped key inside a string value comes first and must not be taken for the field.
        let line = r#"{"src":"a \"name\":\"x\" \\ \u0007 \/ é","reason":"compiler-artifact","target":{"kind":["bin"],"name":"bead"},"executable":"/t/debug/bead","fresh":true}"#;
        assert_eq!(json_field(line, "name").as_deref(), Some("bead"));
        assert_eq!(
            json_field(line, "executable").as_deref(),
            Some("/t/debug/bead")
        );
        assert_eq!(
            json_field(line, "src").as_deref(),
            Some("a \"name\":\"x\" \\ \u{7} / é")
        );
        assert_eq!(json_field(line, "fresh"), None, "not a string");
        assert_eq!(json_field(r#"{"executable":null}"#, "executable"), None);
        assert_eq!(json_field(r#"{"a":"open"#, "a"), None, "unterminated");
        assert_eq!(json_field(r#"{"a":"\u12"}"#, "a"), None, "short escape");
    }

    #[test]
    fn the_root_holds_the_workspace_and_the_infra_drivers() {
        let root = root();
        assert!(root.join("Cargo.toml").is_file());
        assert!(
            root.join("crates")
                .join("beadroll")
                .join("Cargo.toml")
                .is_file()
        );
        assert!(root.join("infra").join("roundtrip.py").is_file());
    }
}
