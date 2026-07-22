//! Bounded little-endian reader for a decompressed MoonBot payload: Delphi primitives,
//! length-prefixed UTF-16LE `WriteStringX` strings, INI lists, and block sub-readers.
//! All lengths are validated BEFORE allocation; an out-of-bounds read returns
//! [`ImportError::Truncated`].

use super::ImportError;

/// Maximum `WriteStringX` length: 2,097,152 UTF-16 code units per the specification.
const MAX_STRING_UNITS: usize = 2 * 1024 * 1024;
/// Maximum entries in one INI section per the specification.
const MAX_INI_ENTRIES: usize = 2048;
/// Reasonable total section limit for one INI list; real blocks contain only a few.
const MAX_INI_SECTIONS: usize = 256;

/// INI-list section containing a name and key/value pairs in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IniSection {
    pub name: String,
    pub entries: Vec<(String, String)>,
}

/// Positional reader over a payload slice that never reads beyond `buf.len()`.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Takes and advances past the next `n` bytes, or returns Truncated with context.
    fn take(&mut self, n: usize, what: &str) -> Result<&'a [u8], ImportError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|e| *e <= self.buf.len())
            .ok_or_else(|| {
                ImportError::Truncated(format!(
                    "{what}: нужно {n} байт, осталось {}",
                    self.remaining()
                ))
            })?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    /// Creates a sub-reader over exactly `size` bytes (a block body).
    ///
    /// Reads cannot escape the block, and the outer reader immediately advances past the
    /// entire block.
    pub fn sub_reader(&mut self, size: usize, what: &str) -> Result<Reader<'a>, ImportError> {
        Ok(Reader::new(self.take(size, what)?))
    }

    pub fn u8(&mut self, what: &str) -> Result<u8, ImportError> {
        Ok(self.take(1, what)?[0])
    }

    /// Reads a Delphi `Boolean`: strictly 0 or 1; any other value is format corruption.
    pub fn bool(&mut self, what: &str) -> Result<bool, ImportError> {
        match self.u8(what)? {
            0 => Ok(false),
            1 => Ok(true),
            v => Err(ImportError::BadValue(format!(
                "{what}: bool должен быть 0/1, получен {v}"
            ))),
        }
    }

    pub fn u16_le(&mut self, what: &str) -> Result<u16, ImportError> {
        let b = self.take(2, what)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn i32_le(&mut self, what: &str) -> Result<i32, ImportError> {
        let b = self.take(4, what)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u32_le(&mut self, what: &str) -> Result<u32, ImportError> {
        let b = self.take(4, what)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn f32_le(&mut self, what: &str) -> Result<f32, ImportError> {
        let b = self.take(4, what)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn f64_le(&mut self, what: &str) -> Result<f64, ImportError> {
        let b = self.take(8, what)?;
        Ok(f64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    /// Reads an `f64` that must be finite; NaN/Infinity in sizes or percentages is corruption.
    pub fn f64_finite(&mut self, what: &str) -> Result<f64, ImportError> {
        let v = self.f64_le(what)?;
        if !v.is_finite() {
            return Err(ImportError::BadValue(format!("{what}: не конечное число")));
        }
        Ok(v)
    }

    /// Reads an `f32` that must be finite.
    pub fn f32_finite(&mut self, what: &str) -> Result<f32, ImportError> {
        let v = self.f32_le(what)?;
        if !v.is_finite() {
            return Err(ImportError::BadValue(format!("{what}: не конечное число")));
        }
        Ok(v)
    }

    /// Reads a `WriteStringX` string: an `i32 LE` UTF-16 code-unit count followed by
    /// `len*2` UTF-16LE bytes.
    ///
    /// A negative or excessive length, truncation, or invalid UTF-16 is an error. The length
    /// is checked against the remaining buffer BEFORE allocation.
    pub fn string_x(&mut self, what: &str) -> Result<String, ImportError> {
        let len = self.i32_le(what)?;
        if len < 0 {
            return Err(ImportError::BadValue(format!(
                "{what}: отрицательная длина строки {len}"
            )));
        }
        let units = len as usize;
        if units > MAX_STRING_UNITS {
            return Err(ImportError::TooLarge(format!(
                "{what}: строка {units} UTF-16 units (лимит {MAX_STRING_UNITS})"
            )));
        }
        let byte_len = units * 2; // units ≤ 2 MiB, so multiplication cannot overflow usize.
        let bytes = self.take(byte_len, what)?;
        let mut buf: Vec<u16> = Vec::with_capacity(units);
        for ch in bytes.chunks_exact(2) {
            buf.push(u16::from_le_bytes([ch[0], ch[1]]));
        }
        String::from_utf16(&buf)
            .map_err(|_| ImportError::BadValue(format!("{what}: некорректный UTF-16")))
    }

    /// Reads an INI list: `i32 section_count`, then `string name`, `i32 entry_count`, and
    /// `entry_count` pairs of `string key`/`string value` for each section.
    ///
    /// Enforces limits and sanity-checks the count against the remaining buffer; each entry
    /// needs at least the two four-byte lengths.
    pub fn ini_list(&mut self, what: &str) -> Result<Vec<IniSection>, ImportError> {
        let section_count = self.i32_le(what)?;
        if section_count < 0 {
            return Err(ImportError::BadValue(format!(
                "{what}: отрицательное число секций {section_count}"
            )));
        }
        let section_count = section_count as usize;
        if section_count > MAX_INI_SECTIONS {
            return Err(ImportError::TooLarge(format!(
                "{what}: {section_count} INI-секций (лимит {MAX_INI_SECTIONS})"
            )));
        }
        let mut sections = Vec::with_capacity(section_count.min(64));
        for si in 0..section_count {
            let name = self.string_x(&format!("{what}: имя секции #{si}"))?;
            let entry_count = self.i32_le(&format!("{what}: [{name}] entry_count"))?;
            if entry_count < 0 {
                return Err(ImportError::BadValue(format!(
                    "{what}: [{name}] отрицательное число записей {entry_count}"
                )));
            }
            let entry_count = entry_count as usize;
            if entry_count > MAX_INI_ENTRIES {
                return Err(ImportError::TooLarge(format!(
                    "{what}: [{name}] {entry_count} записей (лимит {MAX_INI_ENTRIES})"
                )));
            }
            // Every entry needs at least two four-byte lengths, so count cannot exceed
            // remaining/8. This prevents allocation from a bogus count in truncated data.
            if entry_count > self.remaining() / 8 {
                return Err(ImportError::Truncated(format!(
                    "{what}: [{name}] заявлено {entry_count} записей, данных не хватает"
                )));
            }
            let mut entries = Vec::with_capacity(entry_count);
            for ei in 0..entry_count {
                let key = self.string_x(&format!("{what}: [{name}] ключ #{ei}"))?;
                let value = self.string_x(&format!("{what}: [{name}] значение #{ei}"))?;
                entries.push((key, value));
            }
            sections.push(IniSection { name, entries });
        }
        Ok(sections)
    }
}

#[cfg(test)]
mod tests;
