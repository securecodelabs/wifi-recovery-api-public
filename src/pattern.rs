use anyhow::{Result, bail, ensure};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Preview {
    pub pattern: String,
    pub password_length_bytes: Option<usize>,
    pub min_password_length_bytes: usize,
    pub max_password_length_bytes: usize,
    pub candidate_count: u64,
    pub lengths: Vec<Length>,
    pub samples: Vec<Sample>,
}

#[derive(Debug, Serialize)]
pub struct Length {
    pub password_length_bytes: usize,
    pub candidate_count: u64,
}

#[derive(Debug, Serialize)]
pub struct Sample {
    pub text: String,
    pub hex: String,
}

fn alphabet(token: u8) -> Result<Vec<u8>> {
    let values = match token {
        b'd' => (b'0'..=b'9').collect(),
        b'l' => (b'a'..=b'z').collect(),
        b'u' => (b'A'..=b'Z').collect(),
        b'h' => b"0123456789abcdef".to_vec(),
        b'H' => b"0123456789ABCDEF".to_vec(),
        b'a' => (32..=126).collect(),
        b'b' => (0..=255).collect(),
        b's' => (32_u8..=126)
            .filter(|value| !value.is_ascii_alphanumeric())
            .collect(),
        b'?' => vec![b'?'],
        _ => bail!("Unsupported pattern token; use ?d ?l ?u ?h ?H ?s ?a ?b or ??"),
    };
    Ok(values)
}

fn positions(pattern: &str) -> Result<Vec<Vec<u8>>> {
    ensure!(pattern.len() <= 256, "Pattern exceeds 256 bytes");
    ensure!(
        !pattern.chars().any(char::is_control),
        "Literal control characters are not allowed"
    );
    let mut source = pattern.bytes();
    let mut sets = Vec::new();
    while let Some(byte) = source.next() {
        let set = if byte == b'?' {
            alphabet(
                source
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("Pattern ends with an incomplete token"))?,
            )?
        } else {
            vec![byte]
        };
        sets.push(set);
    }
    ensure!(
        (8..=63).contains(&sets.len()),
        "Pattern must produce 8-63 password bytes"
    );
    Ok(sets)
}

fn bounds(
    length: usize,
    increment: bool,
    min: Option<u8>,
    max: Option<u8>,
) -> Result<(usize, usize)> {
    if !increment {
        ensure!(
            min.is_none() && max.is_none(),
            "Increment bounds require increment=true"
        );
        return Ok((length, length));
    }
    let min = usize::from(min.unwrap_or(8));
    let max = usize::from(max.unwrap_or(63));
    ensure!(
        (8..=63).contains(&min) && (min..=63).contains(&max),
        "Increment range must be ordered within 8-63 password bytes"
    );
    ensure!(min <= length, "Pattern is shorter than increment_min");
    Ok((min, max.min(length)))
}

pub fn preview(pattern: &str) -> Result<Preview> {
    preview_range(pattern, false, None, None)
}

pub fn preview_range(
    pattern: &str,
    increment: bool,
    min: Option<u8>,
    max: Option<u8>,
) -> Result<Preview> {
    let sets = positions(pattern)?;
    let (min, max) = bounds(sets.len(), increment, min, max)?;
    let mut count = 1_u64;
    let mut total = 0_u64;
    let mut lengths = Vec::new();
    let mut samples = Vec::new();
    for (index, set) in sets.iter().take(max).enumerate() {
        count = count
            .checked_mul(set.len() as u64)
            .ok_or_else(|| anyhow::anyhow!("Pattern candidate count exceeds u64"))?;
        let length = index + 1;
        if length < min {
            continue;
        }
        total = total
            .checked_add(count)
            .ok_or_else(|| anyhow::anyhow!("Combined pattern candidate count exceeds u64"))?;
        lengths.push(Length {
            password_length_bytes: length,
            candidate_count: count,
        });
        for offset in 0..count.min((3 - samples.len()) as u64) {
            samples.push(sample(&sets[..length], offset));
        }
    }
    Ok(Preview {
        pattern: pattern.into(),
        password_length_bytes: (min == max).then_some(min),
        min_password_length_bytes: min,
        max_password_length_bytes: max,
        candidate_count: total,
        lengths,
        samples,
    })
}

pub fn normalize(search: &mut crate::model::Search) -> Result<()> {
    let preview = preview_range(
        &search.pattern,
        search.increment,
        search.increment_min,
        search.increment_max,
    )?;
    if search.increment {
        search.increment_min = Some(preview.min_password_length_bytes.try_into()?);
        search.increment_max = Some(preview.max_password_length_bytes.try_into()?);
    }
    Ok(())
}

fn sample(sets: &[Vec<u8>], mut index: u64) -> Sample {
    let mut output = Vec::with_capacity(sets.len());
    for set in sets.iter().rev() {
        output.push(set[index as usize % set.len()]);
        index /= set.len() as u64;
    }
    output.reverse();
    Sample {
        text: crate::model::display_bytes(&output),
        hex: hex::encode(output),
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize, preview, preview_range};
    use crate::model::Search;

    #[test]
    fn incremental_counts_boundaries_and_byte_prefixes() {
        let value = preview_range("Home?d?d?d?d?d?d", true, Some(8), Some(10)).unwrap();
        assert_eq!(value.password_length_bytes, None);
        assert_eq!(value.candidate_count, 1_110_000);
        assert_eq!(
            value
                .lengths
                .iter()
                .map(|v| (v.password_length_bytes, v.candidate_count))
                .collect::<Vec<_>>(),
            vec![(8, 10_000), (9, 100_000), (10, 1_000_000)]
        );
        assert_eq!(value.samples[0].text, "Home0000");
        let value = preview_range("abcdefgé?d", true, Some(8), Some(63)).unwrap();
        assert_eq!(value.candidate_count, 12);
        assert_eq!(value.samples[0].hex, "61626364656667c3");
        assert_eq!(value.samples[1].hex, "61626364656667c3a9");
        assert_eq!(value.max_password_length_bytes, 10);
        assert_eq!(
            preview_range(&"?d".repeat(20), true, Some(8), Some(19))
                .unwrap()
                .candidate_count,
            11_111_111_111_100_000_000
        );
    }

    #[test]
    fn invalid_ranges_and_combined_overflow_are_rejected() {
        for (enabled, min, max) in [
            (false, Some(8), None),
            (false, None, Some(9)),
            (true, Some(7), None),
            (true, Some(10), Some(9)),
            (true, Some(8), Some(64)),
            (true, Some(11), None),
            (true, None, Some(7)),
        ] {
            assert!(preview_range("Home?d?d?d?d?d?d", enabled, min, max).is_err());
        }
        assert!(preview_range(&"?d".repeat(20), true, None, None).is_err());
        // A 10^19 prefix followed by two literal suffix bytes overflows only the sum.
        assert!(
            preview_range(&format!("{}ab", "?d".repeat(19)), true, Some(19), Some(21)).is_err()
        );
    }

    #[test]
    fn normalization_preserves_old_fingerprints_and_canonicalizes_ranges() {
        let legacy = r#"{"pattern":"password","runtime_seconds":0,"candidate_limit":0}"#;
        let mut search: Search = serde_json::from_str(legacy).unwrap();
        normalize(&mut search).unwrap();
        assert_eq!(serde_json::to_string(&search).unwrap(), legacy);
        search.increment = true;
        normalize(&mut search).unwrap();
        assert_eq!(
            (search.increment_min, search.increment_max),
            (Some(8), Some(8))
        );
        let mut explicit = search.clone();
        explicit.increment_max = Some(63);
        normalize(&mut explicit).unwrap();
        assert_eq!(search, explicit);
    }

    #[test]
    fn preserves_order_literals_and_byte_lengths() {
        let value = preview("Home?d?d?d?d").unwrap();
        assert_eq!(value.candidate_count, 10_000);
        assert_eq!(value.samples[2].text, "Home0002");
        assert_eq!(preview("pass???d?d?d").unwrap().samples[0].text, "pass?000");
        assert_eq!(preview("école?d?d").unwrap().password_length_bytes, Some(8));
        assert_eq!(
            preview("prefixx?b").unwrap().samples[0].hex,
            "7072656669787800"
        );
    }

    #[test]
    fn validates_all_sets_and_overflow() {
        for (token, size) in [
            ('l', 26),
            ('u', 26),
            ('h', 16),
            ('H', 16),
            ('s', 33),
            ('a', 95),
            ('b', 256),
            ('?', 1),
        ] {
            assert_eq!(
                preview(&format!("prefixx?{token}"))
                    .unwrap()
                    .candidate_count,
                size
            );
        }
        for invalid in [
            "short",
            "password?",
            "password?1",
            "pass\nword",
            "?b?b?b?b?b?b?b?b",
        ] {
            assert!(preview(invalid).is_err(), "{invalid:?}");
        }
        assert!(preview(&"x".repeat(64)).is_err());
        assert_eq!(
            preview(&"?d".repeat(19)).unwrap().candidate_count,
            10_000_000_000_000_000_000
        );
    }
}
