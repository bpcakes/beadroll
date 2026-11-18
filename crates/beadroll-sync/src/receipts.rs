//! Receipt retirement is distinct from attempt settlement: sync keeps accepted receipts so a
//! retry can recognise them. A new successful command retires its actor's older requests.

use std::path::{Path, PathBuf};

use beadroll_core::json::Value;
use beadroll_git::IoError;
use beadroll_store::{Actor, Workspace, json_text, layout, read, receipt_files, remove};

use crate::error::Error;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Receipt {
    /// Gone since it was listed: nothing to settle or remove.
    Gone,
    Malformed,
    Foreign,
    Pending,
    Settled,
}

impl Receipt {
    /// What the receipt at `path` is to `actor`. One that cannot be read is an error, never
    /// junk: under the lock it is a receipt a retry may need, as `load_pending` keeps its attempt.
    fn read(actor: &Actor, path: &Path) -> Result<Self, Error> {
        let data = match std::fs::read(path) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Self::Gone),
            Err(error) => return Err(IoError::at(&error, path).into()),
        };
        let Ok(Value::Object(recorded)) = json_text(&data) else {
            return Ok(Self::Malformed);
        };
        // An actor containing invalid UTF-8 cannot have written a receipt.
        if !actor.writable || recorded.get("actor") != Some(&actor.value) {
            return Ok(Self::Foreign);
        }
        if path.with_extension("json").exists() {
            return Ok(Self::Pending);
        }
        Ok(Self::Settled)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Acceptance {
    Accepted,
    Unaccepted,
}

impl Acceptance {
    fn read(claims: &Path, path: &Path) -> Self {
        let name = path.with_extension("json");
        if claims.join(name.file_name().unwrap_or_default()).exists() {
            Self::Accepted
        } else {
            Self::Unaccepted
        }
    }

    fn retirement(
        self,
        path: &Path,
        latest: Option<&Path>,
        signature: &str,
    ) -> Result<Retirement, Error> {
        if latest.is_some_and(|latest| path.file_name() < latest.file_name()) {
            Ok(Retirement::Now)
        } else if self == Self::Accepted && read(path)? == signature.as_bytes() {
            Ok(Retirement::Recognized)
        } else {
            Ok(Retirement::AfterSuccess)
        }
    }
}

enum Retirement {
    Now,
    Recognized,
    AfterSuccess,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Request {
    New,
    Accepted,
}

#[derive(Default)]
pub(crate) struct RetireAfterSuccess(Vec<PathBuf>);

impl RetireAfterSuccess {
    pub(crate) fn retire(&self) -> Result<(), Error> {
        forget(&self.0)
    }
}

pub(crate) struct SettledCommands {
    pub(crate) request: Request,
    pub(crate) retire: RetireAfterSuccess,
}

/// Classifies under the store lock. Malformed receipts are removed during the initial scan,
/// before decisions about settled receipts, preserving the order of fallible file operations.
pub(crate) fn settle(
    ws: &Workspace,
    replica: &str,
    actor: &Actor,
    signature: &str,
) -> Result<SettledCommands, Error> {
    let mut settled = Vec::new();
    for path in receipt_files(ws) {
        let receipt = Receipt::read(actor, &path)?;
        match receipt {
            Receipt::Malformed => remove(&path)?,
            Receipt::Gone | Receipt::Foreign | Receipt::Pending => {}
            Receipt::Settled => settled.push(path),
        }
    }
    let claims = ws.events_dir.join(replica).join(layout::CLAIMS);
    let settled: Vec<_> = settled
        .into_iter()
        .map(|path| {
            let acceptance = Acceptance::read(&claims, &path);
            (path, acceptance)
        })
        .collect();
    // Names are attempt ids (UUIDv7), so this is time order within this actor's receipts.
    let latest = settled
        .iter()
        .rev()
        .find(|(_, receipt)| *receipt == Acceptance::Accepted)
        .map(|(path, _)| path.as_path());
    let mut result = SettledCommands {
        request: Request::New,
        retire: RetireAfterSuccess::default(),
    };
    for (path, receipt) in &settled {
        match receipt.retirement(path, latest, signature)? {
            Retirement::Now => remove(path)?,
            Retirement::Recognized => {
                result.request = Request::Accepted;
                remove(path)?;
            }
            Retirement::AfterSuccess => result.retire.0.push(path.clone()),
        }
    }
    Ok(result)
}

/// Removes these receipts: the actor has moved on from them.
pub(crate) fn forget(receipts: &[PathBuf]) -> Result<(), Error> {
    receipts.iter().try_for_each(|receipt| remove(receipt))?;
    Ok(())
}

/// A later command of this actor went through: the receipts that would answer a repeat of an
/// earlier command go. Attempts still pending stay for sync to settle.
pub(crate) fn forget_receipts(ws: &Workspace, actor: &Value) -> Result<(), Error> {
    for receipt in receipt_files(ws) {
        let Ok(data) = std::fs::read(&receipt) else {
            continue;
        };
        let Ok(Value::Object(recorded)) = json_text(&data) else {
            continue;
        };
        if recorded.get("actor") == Some(actor) {
            remove(&receipt)?;
        }
    }
    Ok(())
}
