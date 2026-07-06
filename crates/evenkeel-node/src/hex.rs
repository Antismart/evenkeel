//! Serde helpers for FNN's hex-encoded integers.
//!
//! FNN v0.8.x serializes `u128`/`u64`/`u32` as `0x`-prefixed lowercase hex
//! strings on the wire. These modules keep the Rust structs in native
//! integers while matching the wire format exactly.

use serde::{Deserialize, Deserializer, Serializer};

fn parse_hex<T>(s: &str) -> Result<T, String>
where
    T: TryFrom<u128>,
{
    let digits = s
        .strip_prefix("0x")
        .ok_or_else(|| format!("expected 0x-prefixed hex, got {s:?}"))?;
    let v = u128::from_str_radix(digits, 16).map_err(|e| format!("bad hex {s:?}: {e}"))?;
    T::try_from(v).map_err(|_| format!("hex value {s:?} out of range"))
}

macro_rules! hex_mod {
    ($mod_name:ident, $ty:ty) => {
        /// Serde `with`-module mapping `0x`-hex strings to native integers.
        pub mod $mod_name {
            use super::*;

            /// Serialize as a `0x`-prefixed lowercase hex string.
            pub fn serialize<S: Serializer>(v: &$ty, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&format!("{:#x}", v))
            }

            /// Deserialize from a `0x`-prefixed hex string.
            pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<$ty, D::Error> {
                let s = String::deserialize(d)?;
                parse_hex::<$ty>(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

hex_mod!(u128_hex, u128);
hex_mod!(u64_hex, u64);
hex_mod!(u32_hex, u32);

macro_rules! hex_opt_mod {
    ($mod_name:ident, $ty:ty) => {
        /// Serde `with`-module for `Option<int>` as an optional `0x`-hex
        /// string. Pair with `#[serde(skip_serializing_if = "Option::is_none",
        /// default)]` on the field.
        pub mod $mod_name {
            use super::*;

            /// Serialize `Some(v)` as a `0x`-hex string.
            pub fn serialize<S: Serializer>(v: &Option<$ty>, s: S) -> Result<S::Ok, S::Error> {
                match v {
                    Some(v) => s.serialize_some(&format!("{:#x}", v)),
                    None => s.serialize_none(),
                }
            }

            /// Deserialize a missing/null field or a `0x`-hex string.
            pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<$ty>, D::Error> {
                let s = Option::<String>::deserialize(d)?;
                s.map(|s| parse_hex::<$ty>(&s).map_err(serde::de::Error::custom))
                    .transpose()
            }
        }
    };
}

hex_opt_mod!(u128_hex_opt, u128);
hex_opt_mod!(u64_hex_opt, u64);

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    struct T128(#[serde(with = "super::u128_hex")] u128);

    #[test]
    fn round_trips_u128() {
        let json = "\"0xba43b7400\"";
        let v: T128 = serde_json::from_str(json).unwrap();
        assert_eq!(v.0, 50_000_000_000);
        assert_eq!(serde_json::to_string(&v).unwrap(), json);
    }

    #[test]
    fn rejects_unprefixed_and_garbage() {
        assert!(serde_json::from_str::<T128>("\"ba43b7400\"").is_err());
        assert!(serde_json::from_str::<T128>("\"0xzz\"").is_err());
        assert!(serde_json::from_str::<T128>("42").is_err());
    }

    #[test]
    fn zero_and_max_round_trip() {
        assert_eq!(serde_json::from_str::<T128>("\"0x0\"").unwrap().0, 0);
        let max = format!("\"{:#x}\"", u128::MAX);
        assert_eq!(serde_json::from_str::<T128>(&max).unwrap().0, u128::MAX);
    }
}
