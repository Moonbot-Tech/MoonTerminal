//! Bounded little-endian reader для распакованного payload MoonBot: примитивы Delphi,
//! строки `WriteStringX` (UTF-16LE с длиной), INI-списки и суб-ридеры блоков.
//! Все длины проверяются ДО выделения памяти; выход за границу — [`ImportError::Truncated`].

use super::ImportError;

/// Лимит одной строки `WriteStringX` — 2 097 152 UTF-16 code units по ТЗ.
const MAX_STRING_UNITS: usize = 2 * 1024 * 1024;
/// Лимит записей в одной INI-секции по ТЗ.
const MAX_INI_ENTRIES: usize = 2048;
/// Разумный общий лимит секций одного INI-списка (в реальных блоках их единицы).
const MAX_INI_SECTIONS: usize = 256;

/// Секция INI-списка: имя + пары ключ/значение в исходном порядке.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IniSection {
    pub name: String,
    pub entries: Vec<(String, String)>,
}

/// Позиционный reader над срезом payload. Никогда не читает за `buf.len()`.
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

    /// Срез следующих `n` байт (с продвижением) — или Truncated с контекстом.
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

    /// Суб-ридер ровно на `size` байт (тело блока): чтение внутри не выйдет за блок,
    /// внешний ридер сразу перескакивает блок целиком.
    pub fn sub_reader(&mut self, size: usize, what: &str) -> Result<Reader<'a>, ImportError> {
        Ok(Reader::new(self.take(size, what)?))
    }

    pub fn u8(&mut self, what: &str) -> Result<u8, ImportError> {
        Ok(self.take(1, what)?[0])
    }

    /// Delphi `Boolean`: строго 0 или 1, всё прочее — порча формата.
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

    /// `f64`, обязанный быть конечным (размеры/проценты: NaN/Infinity — порча).
    pub fn f64_finite(&mut self, what: &str) -> Result<f64, ImportError> {
        let v = self.f64_le(what)?;
        if !v.is_finite() {
            return Err(ImportError::BadValue(format!("{what}: не конечное число")));
        }
        Ok(v)
    }

    /// `f32`, обязанный быть конечным.
    pub fn f32_finite(&mut self, what: &str) -> Result<f32, ImportError> {
        let v = self.f32_le(what)?;
        if !v.is_finite() {
            return Err(ImportError::BadValue(format!("{what}: не конечное число")));
        }
        Ok(v)
    }

    /// Строка `WriteStringX`: `i32 LE` число UTF-16 code units + `len*2` байт UTF-16LE.
    /// Отрицательная длина, превышение лимита, обрыв или невалидный UTF-16 — ошибка.
    /// Длина сверяется с остатком буфера ДО выделения.
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
        let byte_len = units * 2; // units ≤ 2 MiB → умножение не переполнит usize
        let bytes = self.take(byte_len, what)?;
        let mut buf: Vec<u16> = Vec::with_capacity(units);
        for ch in bytes.chunks_exact(2) {
            buf.push(u16::from_le_bytes([ch[0], ch[1]]));
        }
        String::from_utf16(&buf)
            .map_err(|_| ImportError::BadValue(format!("{what}: некорректный UTF-16")))
    }

    /// INI-список: `i32 section_count`, затем на секцию `string name`, `i32 entry_count`
    /// и `entry_count` пар `string key`/`string value`. Лимиты и sanity-проверка count
    /// против остатка буфера (каждая entry занимает минимум 8 байт двух длин).
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
            // Каждая entry — минимум две 4-байтовые длины: count не может превышать
            // остаток/8. Отсекает выделение по фиктивному count на обрезанных данных.
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
mod tests {
    use super::*;

    /// Тест-хелперы сериализации (двойник Delphi-writer'а для юнитов).
    pub(crate) fn push_string(out: &mut Vec<u8>, s: &str) {
        let units: Vec<u16> = s.encode_utf16().collect();
        out.extend_from_slice(&(units.len() as i32).to_le_bytes());
        for u in units {
            out.extend_from_slice(&u.to_le_bytes());
        }
    }

    #[test]
    fn primitives_roundtrip() {
        let mut buf = Vec::new();
        buf.push(1u8); // bool true
        buf.push(0u8); // bool false
        buf.extend_from_slice(&0xBEEFu16.to_le_bytes());
        buf.extend_from_slice(&(-42i32).to_le_bytes());
        buf.extend_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        buf.extend_from_slice(&1.5f32.to_le_bytes());
        buf.extend_from_slice(&(-2.25f64).to_le_bytes());
        let mut r = Reader::new(&buf);
        assert!(r.bool("b1").unwrap());
        assert!(!r.bool("b2").unwrap());
        assert_eq!(r.u16_le("w").unwrap(), 0xBEEF);
        assert_eq!(r.i32_le("i").unwrap(), -42);
        assert_eq!(r.u32_le("c").unwrap(), 0xDEAD_BEEF);
        assert_eq!(r.f32_le("f").unwrap(), 1.5);
        assert_eq!(r.f64_le("d").unwrap(), -2.25);
        assert!(r.is_empty());
    }

    #[test]
    fn invalid_bool_rejected() {
        let buf = [2u8];
        assert!(matches!(
            Reader::new(&buf).bool("x"),
            Err(ImportError::BadValue(_))
        ));
    }

    #[test]
    fn truncated_primitive() {
        let buf = [0u8; 3];
        assert!(matches!(
            Reader::new(&buf).i32_le("x"),
            Err(ImportError::Truncated(_))
        ));
    }

    #[test]
    fn string_roundtrip_including_cyrillic() {
        let mut buf = Vec::new();
        push_string(&mut buf, "Привет, 世界! ok");
        push_string(&mut buf, "");
        let mut r = Reader::new(&buf);
        assert_eq!(r.string_x("s").unwrap(), "Привет, 世界! ok");
        assert_eq!(r.string_x("e").unwrap(), "");
        assert!(r.is_empty());
    }

    #[test]
    fn negative_and_oversized_string_len() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(-1i32).to_le_bytes());
        assert!(matches!(
            Reader::new(&buf).string_x("s"),
            Err(ImportError::BadValue(_))
        ));
        let mut buf = Vec::new();
        buf.extend_from_slice(&(MAX_STRING_UNITS as i32 + 1).to_le_bytes());
        assert!(matches!(
            Reader::new(&buf).string_x("s"),
            Err(ImportError::TooLarge(_))
        ));
        // Длина валидная, но данных нет — Truncated, БЕЗ выделения буфера под неё.
        let mut buf = Vec::new();
        buf.extend_from_slice(&1000i32.to_le_bytes());
        assert!(matches!(
            Reader::new(&buf).string_x("s"),
            Err(ImportError::Truncated(_))
        ));
    }

    #[test]
    fn invalid_utf16_rejected() {
        // Одинокий high surrogate 0xD800.
        let mut buf = Vec::new();
        buf.extend_from_slice(&1i32.to_le_bytes());
        buf.extend_from_slice(&0xD800u16.to_le_bytes());
        assert!(matches!(
            Reader::new(&buf).string_x("s"),
            Err(ImportError::BadValue(_))
        ));
    }

    #[test]
    fn ini_roundtrip() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&2i32.to_le_bytes());
        push_string(&mut buf, "ColorsLight");
        buf.extend_from_slice(&2i32.to_le_bytes());
        push_string(&mut buf, "graphBK");
        push_string(&mut buf, "16777215");
        push_string(&mut buf, "CandleGreen");
        push_string(&mut buf, "65280");
        push_string(&mut buf, "Empty");
        buf.extend_from_slice(&0i32.to_le_bytes());
        let mut r = Reader::new(&buf);
        let sections = r.ini_list("theme").unwrap();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].name, "ColorsLight");
        assert_eq!(
            sections[0].entries[0],
            ("graphBK".to_string(), "16777215".to_string())
        );
        assert_eq!(sections[1].name, "Empty");
        assert!(sections[1].entries.is_empty());
        assert!(r.is_empty());
    }

    #[test]
    fn ini_entry_count_vs_remaining() {
        // Заявлено 1000 записей, но данных нет — Truncated по sanity-проверке count.
        let mut buf = Vec::new();
        buf.extend_from_slice(&1i32.to_le_bytes());
        push_string(&mut buf, "S");
        buf.extend_from_slice(&1000i32.to_le_bytes());
        assert!(matches!(
            Reader::new(&buf).ini_list("ini"),
            Err(ImportError::Truncated(_))
        ));
    }

    #[test]
    fn ini_limits() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(MAX_INI_SECTIONS as i32 + 1).to_le_bytes());
        assert!(matches!(
            Reader::new(&buf).ini_list("ini"),
            Err(ImportError::TooLarge(_))
        ));
        let mut buf = Vec::new();
        buf.extend_from_slice(&(-5i32).to_le_bytes());
        assert!(matches!(
            Reader::new(&buf).ini_list("ini"),
            Err(ImportError::BadValue(_))
        ));
    }

    #[test]
    fn sub_reader_bounds() {
        let buf = [1u8, 2, 3, 4, 5];
        let mut r = Reader::new(&buf);
        let mut sub = r.sub_reader(3, "block").unwrap();
        assert_eq!(sub.u8("a").unwrap(), 1);
        // Чтение через конец суб-блока — ошибка, даже если у родителя байты есть.
        assert!(matches!(sub.i32_le("b"), Err(ImportError::Truncated(_))));
        // Родитель уже за блоком.
        assert_eq!(r.u8("c").unwrap(), 4);
        // Суб-блок больше остатка родителя.
        assert!(matches!(
            r.sub_reader(10, "d"),
            Err(ImportError::Truncated(_))
        ));
    }
}
