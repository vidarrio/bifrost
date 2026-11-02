//! Generated ids for seasons, games, slots and seats: random, so a URL or a form
//! says nothing about what else exists. `Copy`, like the integers they replaced.

use std::fmt;
use std::str::FromStr;

use argon2::password_hash::rand_core::{OsRng, RngCore};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::sqlite::{Sqlite, SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef};

/// How many characters an id has: 16 of 32 symbols is 80 random bits.
const LEN: usize = 16;
/// Lowercase letters and digits, without `0`, `1`, `l` and `o`, which read alike.
const ALPHABET: &[u8; 32] = b"23456789abcdefghijkmnpqrstuvwxyz";

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id([u8; LEN]);

impl Id {
    pub fn new() -> Self {
        let mut raw = [0u8; LEN];
        OsRng.fill_bytes(&mut raw);
        Id(raw.map(|b| ALPHABET[(b & 31) as usize]))
    }

    pub fn as_str(&self) -> &str {
        // Only ever built from `ALPHABET`, which is ASCII.
        std::str::from_utf8(&self.0).unwrap_or("")
    }
}

impl Default for Id {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Id {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let raw: [u8; LEN] = s.as_bytes().try_into().map_err(|_| "not an id")?;
        if raw.iter().all(|b| ALPHABET.contains(b)) {
            Ok(Id(raw))
        } else {
            Err("not an id")
        }
    }
}

impl serde::Serialize for Id {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for Id {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl sqlx::Type<Sqlite> for Id {
    fn type_info() -> SqliteTypeInfo {
        <str as sqlx::Type<Sqlite>>::type_info()
    }
    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <str as sqlx::Type<Sqlite>>::compatible(ty)
    }
}

impl<'q> sqlx::Encode<'q, Sqlite> for Id {
    fn encode_by_ref(&self, buf: &mut Vec<SqliteArgumentValue<'q>>) -> Result<IsNull, BoxDynError> {
        <String as sqlx::Encode<'q, Sqlite>>::encode(self.to_string(), buf)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for Id {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let s = <&str as sqlx::Decode<'r, Sqlite>>::decode(value)?;
        Ok(s.parse::<Id>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_differ() {
        let (a, b) = (Id::new(), Id::new());
        assert_ne!(a, b);
        assert_eq!(a.to_string().len(), LEN);
        assert_eq!(a.to_string().parse::<Id>(), Ok(a));
    }

    #[test]
    fn only_real_ids_parse() {
        for bad in [
            "",
            "1",
            "12",
            "0123456789abcde",
            "ABCDEFGHJKMNPQRS",
            "abcdefghjkmnpqr/",
            "abcdefghjkmnpqrst",
        ] {
            assert!(bad.parse::<Id>().is_err(), "{bad}");
        }
    }
}
