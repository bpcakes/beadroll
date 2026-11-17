//! What a test writes into the shell scripts it runs or puts on `PATH`: a word quoted for `sh`,
//! and the git a fake `git` hands on to.

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

/// `word` quoted for `sh`: in single quotes, each `'` in it closed, escaped and reopened, so the
/// shell reads it back as it is. A word that is not UTF-8 fails the test: a script is text.
pub fn shell_quoted(word: impl AsRef<OsStr>) -> String {
    let word = word.as_ref();
    let text = word
        .to_str()
        .unwrap_or_else(|| panic!("{word:?} is not UTF-8"));
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The git the test's `PATH` finds: the first executable file named `git` on it, which a fake
/// `git` put earlier on a bead's `PATH` runs.
pub fn git_on_path() -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let runnable = |git: &PathBuf| {
        std::fs::metadata(git)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    };
    std::env::split_paths(&path)
        .map(|dir| dir.join("git"))
        .find(runnable)
        .expect("git on PATH")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::shell_quoted;

    /// A quoted word is read back by `sh` as it was, a single quote included.
    #[test]
    fn sh_reads_a_quoted_word_back_as_it_was() {
        for word in ["plain", "with space", "it's", "'", "$HOME `x` \\n"] {
            let echoed = std::process::Command::new("sh")
                .args(["-c", &format!("printf %s {}", shell_quoted(word))])
                .output()
                .unwrap();
            assert_eq!(String::from_utf8(echoed.stdout).unwrap(), word);
        }
        assert_eq!(shell_quoted(Path::new("/a b")), "'/a b'");
    }
}
