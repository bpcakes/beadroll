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

/// The Python test modules: every `tests/test_*.py`, by name, sorted.
pub(crate) fn test_modules(root: &Path) -> Result<Vec<String>, Failure> {
    let tests = root.join("tests");
    let listing = std::fs::read_dir(&tests).map_err(|e| Failure::io("cannot list", &tests, &e))?;
    let mut modules = Vec::new();
    for entry in listing {
        let name = entry
            .map_err(|e| Failure::io("cannot list", &tests, &e))?
            .file_name();
        if let Some(module) = name.to_str().and_then(|n| n.strip_suffix(".py"))
            && module.starts_with("test_")
        {
            modules.push(module.to_owned());
        }
    }
    modules.sort();
    Ok(modules)
}

/// The Python that runs the test suite, as `check.sh` finds it.
pub(crate) fn python() -> OsString {
    OsString::from("python3")
}

/// The cargo that runs xtask, for the commands that build.
pub(crate) fn cargo() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"))
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
    fn the_root_holds_the_launcher_and_the_tests() {
        let root = root();
        assert!(root.join("bead").is_file());
        assert!(root.join("tests").join("support.py").is_file());
    }
}
