//! Source coordinates use decoded UTF-8; source identity always hashes the original bytes.
//! UTF-16 is admitted only for structural paths, with an explicit BOM and strict text validation.

use std::io::{self, Read};
use std::path::Path;

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "source is not supported text")
}

fn structural_character(character: char) -> bool {
    !matches!(character as u32, 0..=8 | 14..=26 | 28..=31)
}

/// Decode a complete structural input without replacing malformed code units.
/// The BOM selects UTF-16 byte order and is not part of decoded coordinates.
/// A bare BOM is refused rather than treating previously unsupported bytes as an empty source.
pub fn decode_structural_text(bytes: &[u8]) -> io::Result<String> {
    let text = decode_source_text(Path::new("source.ps1"), bytes)?;
    if !text.chars().all(structural_character) {
        return Err(invalid());
    }
    Ok(text)
}

/// Preserve ordinary UTF-8 reads; only structural routes may interpret a UTF-16 BOM.
pub fn decode_source_text(path: &Path, bytes: &[u8]) -> io::Result<String> {
    let mut text = String::new();
    source_text_reader(bytes, path)?.read_to_string(&mut text)?;
    Ok(text)
}

/// A bounded caller can stream decoded UTF-8 without retaining the complete source.
/// The reader does not hash text: callers keep their raw-byte identity and read fences.
pub fn source_text_reader<R: Read>(reader: R, path: &Path) -> io::Result<SourceTextReader<R>> {
    let mut reader = io::BufReader::new(reader);
    let mut prefix = [0; 2];
    let mut prefix_len = 0;
    while prefix_len < prefix.len() {
        match reader.read(&mut prefix[prefix_len..]) {
            Ok(0) => break,
            Ok(count) => prefix_len += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    let little_endian = if prefix_len == 2
        && crate::language_support::is_structural_source_path(&path.to_string_lossy())
    {
        match prefix {
            [0xff, 0xfe] => Some(true),
            [0xfe, 0xff] => Some(false),
            _ => None,
        }
    } else {
        None
    };
    Ok(SourceTextReader {
        reader,
        prefix,
        prefix_len: if little_endian.is_some() {
            0
        } else {
            prefix_len
        },
        prefix_position: 0,
        little_endian,
        pending: [0; 4],
        pending_position: 0,
        pending_len: 0,
        had_unit: false,
    })
}

pub struct SourceTextReader<R: Read> {
    reader: io::BufReader<R>,
    prefix: [u8; 2],
    prefix_len: usize,
    prefix_position: usize,
    little_endian: Option<bool>,
    pending: [u8; 4],
    pending_position: usize,
    pending_len: usize,
    had_unit: bool,
}

impl<R: Read> SourceTextReader<R> {
    fn unit(&mut self) -> io::Result<Option<u16>> {
        let mut bytes = [0; 2];
        let mut count = 0;
        while count < 2 {
            match self.reader.read(&mut bytes[count..]) {
                Ok(0) if count == 0 => return Ok(None),
                Ok(0) => return Err(invalid()),
                Ok(read) => count += read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(Some(if self.little_endian == Some(true) {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        }))
    }
}

impl<R: Read> Read for SourceTextReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.little_endian.is_none() {
            if self.prefix_position < self.prefix_len {
                let count = output.len().min(self.prefix_len - self.prefix_position);
                output[..count].copy_from_slice(
                    &self.prefix[self.prefix_position..self.prefix_position + count],
                );
                self.prefix_position += count;
                return Ok(count);
            }
            return self.reader.read(output);
        }
        if self.pending_position == self.pending_len {
            let Some(first) = self.unit()? else {
                return if self.had_unit { Ok(0) } else { Err(invalid()) };
            };
            self.had_unit = true;
            let value = if (0xd800..=0xdbff).contains(&first) {
                let second = self.unit()?.ok_or_else(invalid)?;
                if !(0xdc00..=0xdfff).contains(&second) {
                    return Err(invalid());
                }
                0x10000 + ((u32::from(first) - 0xd800) << 10) + (u32::from(second) - 0xdc00)
            } else {
                u32::from(first)
            };
            let character = char::from_u32(value)
                .filter(|c| structural_character(*c))
                .ok_or_else(invalid)?;
            self.pending_len = character.encode_utf8(&mut self.pending).len();
            self.pending_position = 0;
        }
        let count = output.len().min(self.pending_len - self.pending_position);
        output[..count]
            .copy_from_slice(&self.pending[self.pending_position..self.pending_position + count]);
        self.pending_position += count;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_bom_text_is_structural_only_and_streams_surrogates() {
        let text = "# café 🦀\r\nnext\n";
        for little in [true, false] {
            let mut bytes = if little {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for unit in text.encode_utf16() {
                bytes.extend(if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            assert_eq!(decode_structural_text(&bytes).unwrap(), text);
            assert!(decode_source_text(Path::new("source.rs"), &bytes).is_err());
            let reader = io::BufReader::with_capacity(1, bytes.as_slice());
            let mut decoded = source_text_reader(reader, Path::new("source.ps1")).unwrap();
            let mut result = Vec::new();
            let mut byte = [0];
            while decoded.read(&mut byte).unwrap() != 0 {
                result.push(byte[0]);
            }
            assert_eq!(result, text.as_bytes());
        }
        for bytes in [
            vec![0xff, 0xfe],
            vec![0xff, 0xfe, 65],
            vec![0xfe, 0xff, 0xdc, 0x00],
            vec![0xff, 0xfe, 0, 0],
            vec![0xff, 0xfe, 1, 0],
            vec![65, 0, 66, 0],
        ] {
            assert!(decode_structural_text(&bytes).is_err(), "{bytes:?}");
        }
        assert_eq!(
            decode_structural_text(b"\xef\xbb\xbf# UTF8\n").unwrap(),
            "\u{feff}# UTF8\n"
        );
        assert_eq!(
            decode_source_text(Path::new("source.rs"), b"a\0b").unwrap(),
            "a\0b"
        );
        assert_eq!(decode_structural_text(b"").unwrap(), "");
    }
}
