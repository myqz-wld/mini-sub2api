//! Count encoded JSON bytes without retaining an encoded buffer or cloning body values.
use serde::Serialize;
use std::io::{self, Write};

pub(crate) fn encoded_len<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<usize> {
    let mut counter = ByteCounter(0);
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.0)
}

#[derive(Default)]
struct ByteCounter(usize);

impl Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("JSON size exceeds addressable memory"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn size_matches_wire_and_canonical_encoding() {
        let controls: String = (0..=31).map(char::from).collect();
        let mut values = vec![
            Value::Null,
            json!(false),
            json!(true),
            json!(i64::MIN),
            json!(u64::MAX),
            json!(-0.0),
            json!(1.25e-100),
            json!(1.25e100),
            json!(""),
            json!("synthetic \"quotes\" \\ slash / <>& 中文 🦀"),
            json!(controls),
            json!([]),
            json!({}),
        ];
        for _ in 0..4 {
            // Deliberately use unsorted, escaped keys. Ordering changes hashes, but never size.
            let nested = json!({"z":values, "\n":true, "a":{"second":2,"first":1}});
            assert_eq!(
                encoded_len(&nested).unwrap(),
                serde_json::to_vec(&nested).unwrap().len()
            );
            assert_eq!(
                encoded_len(&nested).unwrap(),
                crate::subscription_index::canonical(&nested).len()
            );
            values = vec![nested];
        }
    }

    #[test]
    fn borrowed_input_and_output_collections_keep_array_accounting() {
        let input = vec![json!({"role":"user","content":"synthetic input"})];
        let output = std::borrow::Cow::Borrowed(&input);
        let expected = crate::subscription_index::canonical(&Value::Array(input.clone())).len();
        assert_eq!(encoded_len(input.as_slice()).unwrap(), expected);
        assert_eq!(encoded_len(&output).unwrap(), expected);
        assert_eq!(encoded_len(&Vec::<Value>::new()).unwrap(), 2);
    }

    #[test]
    fn large_borrowed_string_counts_utf8_and_escapes() {
        let content = "a\n🦀".repeat(256 * 1024);
        let input = vec![Value::String(content)];
        // Two array delimiters, two quotes, and seven encoded bytes per repeated unit.
        assert_eq!(encoded_len(&input).unwrap(), 4 + 7 * 256 * 1024);
    }

    #[test]
    fn size_errors_propagate_instead_of_returning_partial_counts() {
        struct Invalid;
        impl Serialize for Invalid {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("synthetic encoding failure"))
            }
        }
        assert!(encoded_len(&Invalid).is_err());
        assert!(ByteCounter(usize::MAX).write(b"x").is_err());
    }
}
