//! What a test writes into the shell scripts it runs or puts on `PATH`: a word quoted for `sh`,
//! the git a fake `git` hands on to, and the block `ulimit -f` counts in.

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

/// The block `ulimit -f` counts in, in bytes, which the shell decides: 512 in dash and in bash's
/// POSIX mode, 1024 in the bash 3.2 that is macOS's `/bin/sh`. Measured by a file written in `dir`
/// under a limit of one block, which stops there.
pub fn ulimit_block(dir: &Path) -> u64 {
    let probe = dir.join("ulimit-block");
    // dd ends at the limit, killed by SIGXFSZ or refused with EFBIG: either way the probe is full
    Command::new("/bin/sh")
        .args([
            "-c",
            "ulimit -f 1; exec dd if=/dev/zero of=\"$1\" bs=4096 count=1",
        ])
        .arg("sh")
        .arg(&probe)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("/bin/sh starts");
    let block = std::fs::metadata(&probe).expect("the probe").len();
    assert!([512, 1024].contains(&block), "a block of {block} bytes");
    block
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
