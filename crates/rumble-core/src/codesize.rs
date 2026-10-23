// SPDX-License-Identifier: EPL-2.0

//! Class-file parsing for the codesize (spec §12.1): the sum of `code_length` of every
//! `Code` attribute of every method, as classic RoboRumble's `codesize.Codesize` counts it.
//!
//! The parser reads the constant pool only to learn which entries are UTF-8 strings (to find
//! the name `Code`); nothing else is resolved. How a whole jar is summed is in
//! [`crate::jar`]: classic's tool counts every entry whose name ends in `.class`
//! (case-insensitive) and also descends into nested `.jar` entries.

/// The newest class file major version accepted (Java 21).
pub const MAX_CLASS_MAJOR: u16 = 65;

/// What the validation needs to know about one class file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClassInfo {
    pub major: u16,
    /// Sum of `code_length` over the methods that have a `Code` attribute.
    pub code_size: u32,
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| "truncated class file".to_string())?;
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn skip(&mut self, n: usize) -> Result<(), String> {
        self.take(n).map(|_| ())
    }

    fn u16(&mut self) -> Result<u16, String> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// Parses a class file far enough to sum its `Code` attribute lengths.
pub fn parse_class(bytes: &[u8]) -> Result<ClassInfo, String> {
    let mut r = Reader { bytes, pos: 0 };
    if r.u32()? != 0xCAFE_BABE {
        return Err("not a class file (bad magic)".to_string());
    }
    r.skip(2)?; // minor
    let major = r.u16()?;
    let count = r.u16()? as usize;
    // Per constant pool slot: whether it is the UTF-8 `Code`. Slot 0 is unused.
    let mut is_code = vec![false; count.max(1)];
    let mut index = 1;
    while index < count {
        let tag = r.take(1)?[0];
        match tag {
            1 => {
                let len = r.u16()? as usize;
                is_code[index] = r.take(len)? == b"Code";
            }
            3 | 4 | 9 | 10 | 11 | 12 | 17 | 18 => r.skip(4)?,
            5 | 6 => {
                r.skip(8)?;
                index += 1; // takes two slots
            }
            7 | 8 | 16 | 19 | 20 => r.skip(2)?,
            15 => r.skip(3)?,
            _ => return Err(format!("bad constant pool tag {tag} at entry {index}")),
        }
        index += 1;
    }
    r.skip(6)?; // access flags, this class, super class
    let interfaces = r.u16()? as usize;
    r.skip(2 * interfaces)?;
    let fields = r.u16()?;
    for _ in 0..fields {
        r.skip(6)?; // access, name, descriptor
        let attributes = r.u16()?;
        for _ in 0..attributes {
            r.skip(2)?;
            let len = r.u32()? as usize;
            r.skip(len)?;
        }
    }
    let methods = r.u16()?;
    let mut code_size: u64 = 0;
    for _ in 0..methods {
        r.skip(6)?;
        let attributes = r.u16()?;
        let mut found = false;
        for _ in 0..attributes {
            let name = r.u16()? as usize;
            let len = r.u32()? as usize;
            let body = r.take(len)?;
            if !found && is_code.get(name).copied().unwrap_or(false) {
                // max_stack u16, max_locals u16, code_length u32
                if body.len() < 8 {
                    return Err("Code attribute too short".to_string());
                }
                let code_length = u32::from_be_bytes([body[4], body[5], body[6], body[7]]);
                if code_length as usize > body.len() - 8 {
                    return Err("Code attribute shorter than its code".to_string());
                }
                code_size += u64::from(code_length);
                found = true;
            }
        }
    }
    Ok(ClassInfo {
        major,
        code_size: u32::try_from(code_size).unwrap_or(u32::MAX),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal class: a constant pool with a Long (two slots), `Code` and `x`, and one
    /// method per entry of `code_lengths` (`None`: without code).
    fn tiny_class(code_lengths: &[Option<u32>]) -> Vec<u8> {
        let mut c = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 65];
        c.extend([0, 5]);
        c.push(5);
        c.extend([0; 8]);
        c.push(1);
        c.extend([0, 4]);
        c.extend(b"Code");
        c.push(1);
        c.extend([0, 1]);
        c.extend(b"x");
        c.extend([0, 0, 0, 0, 0, 0]); // access, this, super
        c.extend([0, 0]); // interfaces
        c.extend([0, 0]); // fields
        c.extend((code_lengths.len() as u16).to_be_bytes());
        for length in code_lengths {
            c.extend([0, 1, 0, 4, 0, 4]); // access, name, descriptor
            match length {
                Some(n) => {
                    c.extend([0, 1]);
                    c.extend([0, 3]); // attribute name: Code
                    c.extend((8 + n + 2 + 2).to_be_bytes());
                    c.extend([0, 1, 0, 1]); // max stack, max locals
                    c.extend(n.to_be_bytes());
                    c.extend(std::iter::repeat_n(0u8, *n as usize));
                    c.extend([0, 0]); // exception table
                    c.extend([0, 0]); // attributes
                }
                None => c.extend([0, 0]),
            }
        }
        c.extend([0, 0]); // class attributes
        c
    }

    #[test]
    fn sums_code_lengths() {
        let class = tiny_class(&[Some(10), None, Some(7)]);
        assert_eq!(
            parse_class(&class),
            Ok(ClassInfo {
                major: 65,
                code_size: 17
            })
        );
        assert_eq!(parse_class(&tiny_class(&[])).unwrap().code_size, 0);
    }

    #[test]
    fn rejects_broken_classes() {
        let class = tiny_class(&[Some(10)]);
        assert!(parse_class(&class[..class.len() - 5]).is_err());
        assert!(parse_class(&class[..3]).is_err());
        let mut bad = class.clone();
        bad[0] = 0;
        assert!(parse_class(&bad).unwrap_err().contains("magic"));
        let mut bad = class;
        bad[10] = 2; // an unknown tag for the first pool entry
        assert!(parse_class(&bad).is_err());
    }
}
