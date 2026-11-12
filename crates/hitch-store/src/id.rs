//! Note ids and dates.
//!
//! Ids are 12 characters of Crockford base32, like `01JC8Z6Q3N4V`: the first
//! ten encode the creation time in milliseconds (as in a ULID), so ids sort
//! by age, and the last two are random. A notes file is checked for clashes
//! before a new id is used.

use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A new note id.
pub fn generate() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut out = encode(ms, 10);
    out.push_str(&encode(random() & 0x3ff, 2));
    out
}

/// The id of a hand-written note with no metadata comment: a hash of its
/// heading and how many earlier notes in the file share that heading. It
/// stays the same across loads, so such a note can be edited by id.
pub fn derived(title: &str, nth: usize) -> String {
    // FNV-1a, so the value doesn't change between Rust releases.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in title.bytes().chain((nth as u64).to_le_bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    encode(hash >> 4, 12)
}

/// Today's date in UTC, as `YYYY-MM-DD`.
pub fn today() -> String {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0);
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

fn encode(mut value: u64, width: usize) -> String {
    let mut buf = vec![b'0'; width];
    for slot in buf.iter_mut().rev() {
        *slot = ALPHABET[(value & 31) as usize];
        value >>= 5;
    }
    String::from_utf8(buf).expect("alphabet is ASCII")
}

fn random() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    h.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    h.finish()
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
/// algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_look_like_the_spec() {
        let id = generate();
        assert_eq!(id.len(), 12);
        assert!(id.bytes().all(|b| ALPHABET.contains(&b)));
        // Time prefix of a ULID made in 2026.
        assert!(id.starts_with("01K") || id.as_str() > "01K", "{id}");
    }

    #[test]
    fn derived_ids_are_stable() {
        assert_eq!(derived("Todo", 0), derived("Todo", 0));
        assert_ne!(derived("Todo", 0), derived("Todo", 1));
        assert_eq!(derived("Todo", 0).len(), 12);
    }

    #[test]
    fn dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_403), (2025, 11, 11));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(today().len(), 10);
    }
}
