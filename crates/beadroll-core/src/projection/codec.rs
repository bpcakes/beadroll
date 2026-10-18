//! The bytes a projection is kept in: counts and numbers as varints, texts with their length, ids
//! as their 16 bytes, JSON values with their kind first; a name or a value events share held once
//! and referred to by its number, so that the state read back shares them as replay's did. An
//! issue's texts, bytes of their own, write each the first time and refer to it after; a part of
//! records (the state, the index) refers to a table of its own instead, written after its records
//! and placed by its first eight bytes ([`Writer::tabled`], [`Reader::tabled`]), so that a record's
//! bytes mean the same wherever they are, and a write-back can copy them (roll-lqjj.44).
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

/// The names and the values events share that a part's records refer to by number, each once, in
/// the order they were first written: the numbers of a table only grow, so a record copied from a
/// part refers to what it referred to there.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Table {
    pub(crate) names: Vec<Arc<str>>,
    pub(crate) values: Vec<Arc<Value>>,
}

/// A table being written: its entries, each name's number by its text, and each value's by the
/// address it is held at and, for one held elsewhere, by its bytes.
#[derive(Default)]
struct Numbering {
    table: Table,
    names: HashMap<Arc<str>, u32>,
    addresses: HashMap<usize, u32>,
    /// Each value's number by its bytes, made the first time a value is not found by its address.
    contents: Option<HashMap<Vec<u8>, u32>>,
}

impl Numbering {
    /// The table `table` with more to come, its entries numbered as they are.
    fn of(table: Table) -> Numbering {
        let names = (table.names.iter().enumerate())
            .map(|(n, name)| (Arc::clone(name), small(n)))
            .collect();
        let addresses = (table.values.iter().enumerate())
            .map(|(n, value)| (Arc::as_ptr(value) as usize, small(n)))
            .collect();
        Numbering {
            table,
            names,
            addresses,
            contents: None,
        }
    }

    fn name(&mut self, name: &Arc<str>) -> u32 {
        if let Some(number) = self.names.get(name) {
            return *number;
        }
        let number = small(self.table.names.len());
        self.table.names.push(Arc::clone(name));
        self.names.insert(Arc::clone(name), number);
        number
    }

    /// `value`'s number: of the entry held at its address, else of the entry with its bytes, else of
    /// a new entry. Every address it knows is of a value it holds or that the projection being
    /// encoded holds, so none is another value's while the table is written.
    fn value(&mut self, value: &Arc<Value>) -> u32 {
        let address = Arc::as_ptr(value) as usize;
        if let Some(number) = self.addresses.get(&address) {
            return *number;
        }
        let values = &self.table.values;
        let contents = self.contents.get_or_insert_with(|| {
            (values.iter().enumerate())
                .map(|(n, value)| (bytes_of(value), small(n)))
                .collect()
        });
        let bytes = bytes_of(value);
        let number = match contents.get(&bytes) {
            Some(number) => *number,
            None => {
                let number = small(self.table.values.len());
                contents.insert(bytes, number);
                self.table.values.push(Arc::clone(value));
                number
            }
        };
        // found by its address from now on: the encode that asks holds it until the table is written
        self.addresses.insert(address, number);
        number
    }
}

/// A value's bytes, as a writer writes it: what tells two values apart in a table.
pub(super) fn bytes_of(value: &Value) -> Vec<u8> {
    let mut w = Writer::default();
    w.value(value);
    w.finish()
}

/// `n` as a number in a table (a projection holds fewer than 2³² of anything).
fn small(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Bytes being written: names and shared values each written the first time and referred to after,
/// or, for a part of records, referred to in its table.
#[derive(Default)]
pub(crate) struct Writer {
    out: Vec<u8>,
    names: HashMap<Arc<str>, u32>,
    shared: HashMap<usize, u32>,
    table: Option<Numbering>,
}

impl Writer {
    /// A part of records referring to `table`, and to what is added to it, by number: its first
    /// eight bytes kept for where the table will be written ([`Writer::table_here`]).
    pub(crate) fn tabled(table: Table, capacity: usize) -> Writer {
        let mut out = Vec::with_capacity(capacity);
        out.extend_from_slice(&[0; 8]);
        Writer {
            out,
            names: HashMap::new(),
            shared: HashMap::new(),
            table: Some(Numbering::of(table)),
        }
    }

    /// Writes the table here, after the part's records, and says so in its first eight bytes: what
    /// follows is read with it. What the table is, as written.
    pub(crate) fn table_here(&mut self) -> Table {
        let at = (self.out.len() as u64).to_le_bytes();
        if let Some(head) = self.out.get_mut(..8) {
            head.copy_from_slice(&at);
        }
        let table = self.table.take().map(|numbering| numbering.table);
        let table = table.unwrap_or_default();
        self.count(table.names.len());
        for name in &table.names {
            self.text(name);
        }
        self.count(table.values.len());
        for value in &table.values {
            self.value(value);
        }
        table
    }

    /// `bytes` as they are: records copied.
    pub(crate) fn copy(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
    }

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

    /// A name, written the first time and referred to after: 0 and the text, or its number + 1; in
    /// a part of records, always its number in the part's table + 1, as a reader that has read the
    /// table first reads it.
    pub(crate) fn name(&mut self, name: &Arc<str>) {
        if let Some(table) = &mut self.table {
            let number = table.name(name);
            return self.number(u64::from(number) + 1);
        }
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
        if let Some(table) = &mut self.table {
            let number = table.value(value);
            return self.number(u64::from(number) + 1);
        }
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
    /// Where `bytes` begin in the part they were taken from.
    base: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader {
            bytes,
            at: 0,
            names: Vec::new(),
            shared: Vec::new(),
            base: 0,
        }
    }

    /// A part [`Writer::tabled`] wrote: a reader of its records, which refer to its table, and one
    /// of what follows the table, and the table. The records' reader holds the table's entries as
    /// it would hold names and values it had read, so it reads their numbers as it reads any.
    pub(crate) fn tabled(bytes: &'a [u8]) -> Result<(Reader<'a>, Reader<'a>, Table), Unreadable> {
        let head: [u8; 8] = (bytes.get(..8))
            .and_then(|head| head.try_into().ok())
            .ok_or(Unreadable("cut short"))?;
        let at = usize::try_from(u64::from_le_bytes(head)).map_err(|_| Unreadable("a table"))?;
        let (records, after) = (bytes.get(8..at), bytes.get(at..));
        let (Some(records), Some(after)) = (records, after) else {
            return Err(Unreadable("a table"));
        };
        let mut after = Reader::new(after);
        after.base = at;
        let mut table = Table::default();
        for _ in 0..after.count()? {
            table.names.push(Arc::from(after.text()?));
        }
        for _ in 0..after.count()? {
            table.values.push(Arc::new(after.value()?));
        }
        let records = Reader {
            bytes: records,
            at: 0,
            names: table.names.clone(),
            shared: table.values.clone(),
            base: 8,
        };
        Ok((records, after, table))
    }

    /// Where it is in the part it reads: what a write-back copies is placed by this.
    pub(crate) fn at(&self) -> usize {
        self.base + self.at
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
