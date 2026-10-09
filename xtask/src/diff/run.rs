//! Running a Python driver over many cases at once, and telling two reports apart.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use beadroll_core::json::{self, Value};

use crate::workspace::{self, Failure};

/// The integer limit the drivers run under: Python's default (`docs/RUST-PORT.md` §5 row 2).
pub(crate) const INT_DIGITS: &str = "4300";

/// The driver's answer to each input line, in order, from `jobs` processes each taking a contiguous
/// share of the lines.
pub(crate) fn python(driver: &Path, lines: &[String], jobs: usize) -> Result<Vec<String>, Failure> {
    let share = lines.len().div_ceil(jobs.max(1)).max(1);
    let answers: Vec<Result<Vec<String>, Failure>> = std::thread::scope(|scope| {
        let running: Vec<_> = lines
            .chunks(share)
            .map(|chunk| scope.spawn(move || one(driver, chunk)))
            .collect();
        running
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| Err(Failure::Failed("a driver thread panicked".into())))
            })
            .collect()
    });
    let mut out = Vec::with_capacity(lines.len());
    for answer in answers {
        out.extend(answer?);
    }
    Ok(out)
}

fn one(driver: &Path, lines: &[String]) -> Result<Vec<String>, Failure> {
    let mut child = Command::new(workspace::python())
        .arg(driver)
        .env("PYTHONINTMAXSTRDIGITS", INT_DIGITS)
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| Failure::io("cannot run", driver, &error))?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let (Some(mut stdin), Some(stdout)) = (stdin, stdout) else {
        return Err(Failure::Failed("the driver has no pipes".into()));
    };
    let answers = std::thread::scope(|scope| {
        let feeder = scope.spawn(move || -> std::io::Result<()> {
            for line in lines {
                stdin.write_all(line.as_bytes())?;
                stdin.write_all(b"\n")?;
            }
            Ok(())
        });
        let answers: std::io::Result<Vec<String>> = BufReader::new(stdout).lines().collect();
        let fed = feeder
            .join()
            .unwrap_or_else(|_| Err(std::io::Error::other("the feeder panicked")));
        (answers, fed)
    });
    let status = child
        .wait()
        .map_err(|error| Failure::io("cannot wait for", driver, &error))?;
    let (answers, fed) = answers;
    let answers =
        answers.map_err(|error| Failure::io("cannot read the answers of", driver, &error))?;
    if !status.success() || fed.is_err() || answers.len() != lines.len() {
        return Err(Failure::Failed(format!(
            "{} failed ({status}) after {} of {} cases",
            driver.display(),
            answers.len(),
            lines.len()
        )));
    }
    Ok(answers)
}

/// Where two reports first differ, and how; `None` when they are the same.
pub(crate) fn difference(python: &str, rust: &str) -> Option<String> {
    if python == rust {
        return None;
    }
    let parse = |text: &str| json::parse(text.as_bytes()).map(|parsed| parsed.value);
    let (Ok(python_value), Ok(rust_value)) = (parse(python), parse(rust)) else {
        return Some(format!(
            "python: {}\n    rust:   {}",
            clip(python),
            clip(rust)
        ));
    };
    let found = first(&python_value, &rust_value, String::from("$"));
    Some(found.unwrap_or_else(|| "the same values, written differently".into()))
}

fn first(python: &Value, rust: &Value, at: String) -> Option<String> {
    match (python, rust) {
        (Value::Object(a), Value::Object(b)) => {
            let keys_a: Vec<&String> = a.keys().collect();
            let keys_b: Vec<&String> = b.keys().collect();
            if keys_a != keys_b {
                return Some(format!(
                    "{at}: keys differ\n    python: {keys_a:?}\n    rust:   {keys_b:?}"
                ));
            }
            a.iter()
                .find_map(|(key, value)| first(value, b.get(key)?, format!("{at}.{key}")))
        }
        (Value::Array(a), Value::Array(b)) => {
            let found = a
                .iter()
                .zip(b)
                .enumerate()
                .find_map(|(n, (x, y))| first(x, y, format!("{at}[{n}]")));
            found.or_else(|| {
                (a.len() != b.len())
                    .then(|| format!("{at}: python has {} items, rust {}", a.len(), b.len()))
            })
        }
        _ if python == rust
            && json::write(python, json::Format::COMPACT).ok()
                == json::write(rust, json::Format::COMPACT).ok() =>
        {
            None
        }
        _ => Some(format!(
            "{at}\n    python: {}\n    rust:   {}",
            clip(&json::write(python, json::Format::COMPACT).unwrap_or_default()),
            clip(&json::write(rust, json::Format::COMPACT).unwrap_or_default())
        )),
    }
}

fn clip(text: &str) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(400).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}
