//! The id of an event its caller needs before emitting it, as a create's key is minted from its
//! id: one only a transaction mints, which the compiler keeps from any other event.

/// A fresh event id for one event, which only a transaction mints ([`Tx::mint`](super::Tx::mint),
/// [`Emitter::mint`](super::Emitter::mint), [`Attempt::mint`](super::Attempt::mint)) and an emit
/// takes by value. Made from no text, it names no event the journal holds, nor one an attempt
/// waiting on its remote holds, nor a record only an integrity note shows; not `Clone`, and
/// consumed by the emit it is given to, it names one event alone. A supplied id of the journal's
/// would put a second event under it, which the journal sets aside with the first.
///
/// ```no_run
/// # use beadroll_core::{json::Object, op::OpName};
/// fn made(tx: &mut beadroll_sync::Tx<'_>, data: Object) -> Result<(), beadroll_sync::Error> {
///     let id = tx.mint();
///     let _key_seed = id.as_str().to_owned();
///     tx.emit(OpName::Create, None, data, Some(id))?;
///     Ok(())
/// }
/// ```
///
/// One is given to one event alone:
///
/// ```compile_fail,E0382
/// # use beadroll_core::{json::Object, op::OpName};
/// fn made(tx: &mut beadroll_sync::Tx<'_>, data: Object) -> Result<(), beadroll_sync::Error> {
///     let id = tx.mint();
///     tx.emit(OpName::Create, None, data.clone(), Some(id))?;
///     tx.emit(OpName::Create, None, data, Some(id))?;
///     Ok(())
/// }
/// ```
///
/// and is made from no text:
///
/// ```compile_fail,E0423
/// let id = beadroll_sync::Minted("0190a8a2-5e9a-7000-8000-000000000000".to_owned());
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct Minted(pub(super) String);

impl Minted {
    /// The id, as text: what a create's key is minted from before it is emitted.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
