//! The bytes a projection is kept in: counts and numbers as varints, texts with their length, ids
//! as their 16 bytes, JSON values with their kind first; a name or a value events share written the
//! first time and referred to after, so that the state read back shares them as replay's did.
//!
//! Nothing read is trusted further than the bytes there are: a count is at most the bytes left,
//! and values nest little deeper than JSON bead reads ([`NESTING`]). A store checks a projection's
//! bytes as a whole before it reads them; this is what keeps a damaged one from being more than
//! unreadable.

use std::collections::HashMap;
use std::sync::Arc;

use crate::id::EventId;
use crate::json::{Int, MAX_DEPTH, Object, Value};

/// How deeply a value read back may nest arrays and objects: as deep as JSON bead reads, and the
/// few levels replay wraps an event's values in (a dependency's metadata, an imported comment).
const NESTING: usize = MAX_DEPTH + 8;

/// Why bytes are no projection: what was being read when they ran out or did not fit.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unreadable projection: {0}")]
pub struct Unreadable(pub(crate) &'static str);

/// Bytes being written.
#[derive(Default)]
pub(crate) struct Writer {
    out: Vec<u8>,
    names: HashMap<Arc<str>, u32>,
    shared: HashMap<usize, u32>,
}

impl Writer {
    /// What was written.
    pub(crate) fn finish(self) -> Vec<u8> {
        self.out
    }

    pub(crate) fn byte(&mut self, byte: u8) {
        self.out.push(byte);
    }

    pub(crate) fn flag(&mut self, flag: bool) {
        self.byte(u8::from(flag));
    }

    /// `n`, seven bits a byte, the last byte's high bit clear.
    pub(crate) fn number(&mut self, mut n: u64) {
        while n >= 0x80 {
            self.out.push((n as u8) | 0x80);
            n >>= 7;
        }
        self.out.push(n as u8);
    }

    pub(crate) fn count(&mut self, n: usize) {
        self.number(n as u64);
    }

    pub(crate) fn text(&mut self, text: &str) {
        self.count(text.len());
        self.out.extend_from_slice(text.as_bytes());
    }

    pub(crate) fn id(&mut self, id: EventId) {
        self.out.extend_from_slice(&id.as_u128().to_be_bytes());
    }

    /// Sixteen bytes as they are: a sum.
    pub(crate) fn sixteen(&mut self, bytes: &[u8; 16]) {
        self.out.extend_from_slice(bytes);
    }

    /// The ids, counted and then written in the order they come: a slice's or a set's own.
    pub(crate) fn ids<'a>(&mut self, ids: impl ExactSizeIterator<Item = &'a EventId>) {
        self.count(ids.len());
        for id in ids {
            self.id(*id);
        }
    }

    pub(crate) fn optional_text(&mut self, text: Option<&str>) {
        self.flag(text.is_some());
        if let Some(text) = text {
            self.text(text);
        }
    }

    /// A name, written the first time and referred to after: 0 and the text, or its number + 1.
    pub(crate) fn name(&mut self, name: &Arc<str>) {
        match self.names.get(name) {
            Some(number) => self.number(u64::from(*number) + 1),
            None => {
                let number = u32::try_from(self.names.len()).unwrap_or(u32::MAX);
                self.names.insert(Arc::clone(name), number);
                self.number(0);
                self.text(name);
            }
        }
    }

    pub(crate) fn optional_name(&mut self, name: Option<&Arc<str>>) {
        self.flag(name.is_some());
        if let Some(name) = name {
            self.name(name);
        }
    }

    /// A value events share, as [`Writer::name`] writes a name.
    pub(crate) fn shared(&mut self, value: &Arc<Value>) {
        let address = Arc::as_ptr(value) as usize;
        match self.shared.get(&address) {
            Some(number) => self.number(u64::from(*number) + 1),
            None => {
                let number = u32::try_from(self.shared.len()).unwrap_or(u32::MAX);
                self.shared.insert(address, number);
                self.number(0);
                self.value(value);
            }
        }
    }

    pub(crate) fn optional_shared(&mut self, value: Option<&Arc<Value>>) {
        self.flag(value.is_some());
        if let Some(value) = value {
            self.shared(value);
        }
    }

    /// A JSON value: its kind, then what it holds.
    pub(crate) fn value(&mut self, value: &Value) {
        match value {
            Value::Null => self.byte(0),
            Value::Bool(false) => self.byte(1),
            Value::Bool(true) => self.byte(2),
            Value::Int(n) => match n.as_i64() {
                Some(small) => {
                    self.byte(3);
                    self.number(((small << 1) ^ (small >> 63)) as u64);
                }
                None => {
                    self.byte(4);
                    self.text(&n.to_string());
                }
            },
            Value::Float(x) => {
                self.byte(5);
                self.out.extend_from_slice(&x.to_bits().to_be_bytes());
            }
            Value::Str(text) => {
                self.byte(6);
                self.text(text);
            }
            Value::Array(items) => {
                self.byte(7);
                self.count(items.len());
                for item in items {
                    self.value(item);
                }
            }
            Value::Object(members) => {
                self.byte(8);
                self.object(members);
            }
        }
    }

    pub(crate) fn object(&mut self, members: &Object) {
        self.count(members.len());
        for (key, value) in members {
            self.text(key);
            self.value(value);
        }
    }

    pub(crate) fn optional_value(&mut self, value: Option<&Value>) {
        self.flag(value.is_some());
        if let Some(value) = value {
            self.value(value);
        }
    }
}

/// Bytes being read.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    names: Vec<Arc<str>>,
    shared: Vec<Arc<Value>>,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader {
            bytes,
            at: 0,
            names: Vec::new(),
            shared: Vec::new(),
        }
    }

    /// Whether every byte was read: a projection's sections are read whole.
    pub(crate) fn done(&self) -> Result<(), Unreadable> {
        if self.at == self.bytes.len() {
            Ok(())
        } else {
            Err(Unreadable("bytes after the end"))
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], Unreadable> {
        let end = self.at.checked_add(n).ok_or(Unreadable("a length"))?;
        let taken = self
            .bytes
            .get(self.at..end)
            .ok_or(Unreadable("cut short"))?;
        self.at = end;
        Ok(taken)
    }

    pub(crate) fn byte(&mut self) -> Result<u8, Unreadable> {
        Ok(self.take(1)?.first().copied().unwrap_or_default())
    }

    pub(crate) fn flag(&mut self) -> Result<bool, Unreadable> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Unreadable("a flag")),
        }
    }

    pub(crate) fn number(&mut self) -> Result<u64, Unreadable> {
        let mut n: u64 = 0;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            n |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(n);
            }
        }
        Err(Unreadable("a number"))
    }

    pub(crate) fn small(&mut self) -> Result<u32, Unreadable> {
        u32::try_from(self.number()?).map_err(|_| Unreadable("a position"))
    }

    /// An index into a list the reader holds.
    pub(crate) fn index(&mut self) -> Result<usize, Unreadable> {
        usize::try_from(self.number()?).map_err(|_| Unreadable("an index"))
    }

    /// A count of things written one byte or more each: never more than the bytes left.
    pub(crate) fn count(&mut self) -> Result<usize, Unreadable> {
        let n = usize::try_from(self.number()?).map_err(|_| Unreadable("a count"))?;
        if n > self.bytes.len() - self.at {
            return Err(Unreadable("a count"));
        }
        Ok(n)
    }

    pub(crate) fn text(&mut self) -> Result<String, Unreadable> {
        let n = self.count()?;
        let bytes = self.take(n)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| Unreadable("a text"))
    }

    pub(crate) fn id(&mut self) -> Result<EventId, Unreadable> {
        let bytes: [u8; 16] = self.take(16)?.try_into().map_err(|_| Unreadable("an id"))?;
        Ok(EventId::from_u128(u128::from_be_bytes(bytes)))
    }

    /// Sixteen bytes as [`Writer::sixteen`] wrote them.
    pub(crate) fn sixteen(&mut self) -> Result<[u8; 16], Unreadable> {
        let bytes = self.take(16)?;
        <[u8; 16]>::try_from(bytes).map_err(|_| Unreadable("a sum"))
    }

    pub(crate) fn ids(&mut self) -> Result<Vec<EventId>, Unreadable> {
        let n = self.count()?;
        (0..n).map(|_| self.id()).collect()
    }

    pub(crate) fn optional_text(&mut self) -> Result<Option<String>, Unreadable> {
        self.flag()?.then(|| self.text()).transpose()
    }

    pub(crate) fn name(&mut self) -> Result<Arc<str>, Unreadable> {
        match self.number()? {
            0 => {
                let name: Arc<str> = Arc::from(self.text()?);
                self.names.push(Arc::clone(&name));
                Ok(name)
            }
            n => (usize::try_from(n - 1).ok())
                .and_then(|at| self.names.get(at))
                .cloned()
                .ok_or(Unreadable("a name")),
        }
    }

    pub(crate) fn optional_name(&mut self) -> Result<Option<Arc<str>>, Unreadable> {
        self.flag()?.then(|| self.name()).transpose()
    }

    pub(crate) fn shared(&mut self) -> Result<Arc<Value>, Unreadable> {
        match self.number()? {
            0 => {
                let value = Arc::new(self.value()?);
                self.shared.push(Arc::clone(&value));
                Ok(value)
            }
            n => (usize::try_from(n - 1).ok())
                .and_then(|at| self.shared.get(at))
                .cloned()
                .ok_or(Unreadable("a shared value")),
        }
    }

    pub(crate) fn optional_shared(&mut self) -> Result<Option<Arc<Value>>, Unreadable> {
        self.flag()?.then(|| self.shared()).transpose()
    }

    pub(crate) fn value(&mut self) -> Result<Value, Unreadable> {
        self.value_within(NESTING)
    }

    /// A value nesting at most `depth` arrays and objects.
    fn value_within(&mut self, depth: usize) -> Result<Value, Unreadable> {
        Ok(match self.byte()? {
            0 => Value::Null,
            1 => Value::Bool(false),
            2 => Value::Bool(true),
            3 => {
                let n = self.number()?;
                Value::Int(Int::from(((n >> 1) as i64) ^ -((n & 1) as i64)))
            }
            4 => Value::Int(Int::parse(&self.text()?).ok_or(Unreadable("an integer"))?),
            5 => {
                let bytes: [u8; 8] = self
                    .take(8)?
                    .try_into()
                    .map_err(|_| Unreadable("a float"))?;
                Value::Float(f64::from_bits(u64::from_be_bytes(bytes)))
            }
            6 => Value::Str(self.text()?),
            7 => {
                let depth = depth.checked_sub(1).ok_or(Unreadable("nesting"))?;
                let n = self.count()?;
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push(self.value_within(depth)?);
                }
                Value::Array(items)
            }
            8 => {
                let depth = depth.checked_sub(1).ok_or(Unreadable("nesting"))?;
                Value::from(self.object_within(depth)?)
            }
            _ => return Err(Unreadable("a value")),
        })
    }

    pub(crate) fn object(&mut self) -> Result<Object, Unreadable> {
        self.object_within(NESTING)
    }

    fn object_within(&mut self, depth: usize) -> Result<Object, Unreadable> {
        let n = self.count()?;
        let mut members = Object::with_capacity(n);
        for _ in 0..n {
            let key = self.text()?;
            let value = self.value_within(depth)?;
            members.insert(key, value);
        }
        Ok(members)
    }

    pub(crate) fn optional_value(&mut self) -> Result<Option<Value>, Unreadable> {
        self.flag()?.then(|| self.value()).transpose()
    }
}
