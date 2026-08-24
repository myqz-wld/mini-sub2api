//! JSON serialization helpers for ASCII-only compatibility headers.

use serde::Serialize;
use std::io;

struct AsciiJsonFormatter;

impl serde_json::ser::Formatter for AsciiJsonFormatter {
    fn write_string_fragment<W>(&mut self, writer: &mut W, fragment: &str) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        let mut start = 0;
        for (index, ch) in fragment.char_indices() {
            if ch.is_ascii() {
                continue;
            }
            if start < index {
                writer.write_all(&fragment.as_bytes()[start..index])?;
            }
            let mut utf16 = [0; 2];
            for code_unit in ch.encode_utf16(&mut utf16) {
                write!(writer, "\\u{code_unit:04x}")?;
            }
            start = index + ch.len_utf8();
        }
        if start < fragment.len() {
            writer.write_all(&fragment.as_bytes()[start..])?;
        }
        Ok(())
    }
}

pub(crate) fn to_ascii_json_string<T>(value: &T) -> serde_json::Result<String>
where
    T: Serialize + ?Sized,
{
    let mut bytes = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, AsciiJsonFormatter);
    value.serialize(&mut serializer)?;
    String::from_utf8(bytes)
        .map_err(|error| serde_json::Error::io(io::Error::new(io::ErrorKind::InvalidData, error)))
}

#[cfg(test)]
mod tests {
    use super::to_ascii_json_string;
    use pretty_assertions::assert_eq;

    #[test]
    fn escapes_non_ascii_json_string_content() {
        let value = serde_json::json!({"path": "/tmp/東京", "emoji": "🚀"});
        let serialized = to_ascii_json_string(&value).expect("ASCII JSON");
        assert_eq!(
            serialized,
            r#"{"path":"/tmp/\u6771\u4eac","emoji":"\ud83d\ude80"}"#
        );
        assert!(serialized.is_ascii());
    }
}
