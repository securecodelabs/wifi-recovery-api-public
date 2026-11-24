use anyhow::{Result, bail, ensure};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Preview {
    pub pattern: String,
    pub password_length_bytes: usize,
    pub candidate_count: u64,
    pub samples: Vec<Sample>,
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

pub fn preview(pattern: &str) -> Result<Preview> {
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
    let count = sets
        .iter()
        .try_fold(1_u64, |total, set| total.checked_mul(set.len() as u64))
        .ok_or_else(|| anyhow::anyhow!("Pattern candidate count exceeds u64"))?;
    let samples = (0..count.min(3))
        .map(|index| sample(&sets, index))
        .collect();
    Ok(Preview {
        pattern: pattern.into(),
        password_length_bytes: sets.len(),
        candidate_count: count,
        samples,
    })
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
    use super::preview;

    #[test]
    fn preserves_order_literals_and_byte_lengths() {
        let value = preview("Home?d?d?d?d").unwrap();
        assert_eq!(value.candidate_count, 10_000);
        assert_eq!(value.samples[2].text, "Home0002");
        assert_eq!(preview("pass???d?d?d").unwrap().samples[0].text, "pass?000");
        assert_eq!(preview("école?d?d").unwrap().password_length_bytes, 8);
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
