//! Crosslink fat-pointer types (byte-compatible with zebra-crosslink / terp-rs LC).
//!
//! Version-4 headers (stock Zcash / mainnet) do not include this field on the
//! wire. Logical version ≥ 5 (Crosslink prototype nets) appends it after Equihash.

use std::io::{Read, Write};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use serde::{Deserialize, Serialize};

use crate::serialization::{SerializationError, ZcashDeserialize, ZcashSerialize};

/// Ed25519 signature + finalizer public key inside a fat pointer.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FatPointerSignature {
    /// Raw 32-byte Ed25519 public key (not hex).
    pub public_key: [u8; 32],
    /// Raw 64-byte Ed25519 signature.
    #[serde(with = "serde_big_array::BigArray")]
    pub vote_signature: [u8; 64],
}

impl FatPointerSignature {
    /// Pack as 96 bytes: pk || sig.
    pub fn to_bytes(&self) -> [u8; 96] {
        let mut buf = [0u8; 96];
        buf[..32].copy_from_slice(&self.public_key);
        buf[32..].copy_from_slice(&self.vote_signature);
        buf
    }

    /// Unpack 96 bytes: pk || sig.
    pub fn from_bytes(bytes: &[u8; 96]) -> Self {
        Self {
            public_key: bytes[..32].try_into().expect("32"),
            vote_signature: bytes[32..].try_into().expect("64"),
        }
    }
}

impl ZcashSerialize for FatPointerSignature {
    fn zcash_serialize<W: Write>(&self, mut writer: W) -> Result<(), std::io::Error> {
        writer.write_all(&self.to_bytes())
    }
}

impl ZcashDeserialize for FatPointerSignature {
    fn zcash_deserialize<R: Read>(mut reader: R) -> Result<Self, SerializationError> {
        let mut buf = [0u8; 96];
        reader.read_exact(&mut buf)?;
        Ok(Self::from_bytes(&buf))
    }
}

/// Signed vote bundle for a BFT block (TFL fat pointer).
///
/// Vote template is 44 bytes: 32-byte BLAKE3 of the BFT block + 12-byte suffix.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FatPointerToBftBlock {
    /// 44-byte vote template (hash || suffix).
    #[serde(with = "serde_big_array::BigArray")]
    pub vote_for_block_without_finalizer_public_key: [u8; 44],
    /// Finalizer signatures.
    pub signatures: Vec<FatPointerSignature>,
}

impl FatPointerToBftBlock {
    /// Empty signatures, zero vote template.
    pub fn null() -> Self {
        Self {
            vote_for_block_without_finalizer_public_key: [0u8; 44],
            signatures: Vec::new(),
        }
    }

    /// BLAKE3 of the BFT block this pointer votes for.
    pub fn points_at_block_hash(&self) -> [u8; 32] {
        self.vote_for_block_without_finalizer_public_key[..32]
            .try_into()
            .expect("44 > 32")
    }

    /// True if at least one signature is present (not a null pointer).
    pub fn has_signatures(&self) -> bool {
        !self.signatures.is_empty()
    }
}

impl Default for FatPointerToBftBlock {
    fn default() -> Self {
        Self::null()
    }
}

impl ZcashSerialize for FatPointerToBftBlock {
    fn zcash_serialize<W: Write>(&self, mut writer: W) -> Result<(), std::io::Error> {
        writer.write_all(&self.vote_for_block_without_finalizer_public_key)?;
        writer.write_u16::<LittleEndian>(self.signatures.len() as u16)?;
        for signature in &self.signatures {
            signature.zcash_serialize(&mut writer)?;
        }
        Ok(())
    }
}

impl ZcashDeserialize for FatPointerToBftBlock {
    fn zcash_deserialize<R: Read>(mut reader: R) -> Result<Self, SerializationError> {
        let mut vote = [0u8; 44];
        reader.read_exact(&mut vote)?;
        let len = reader.read_u16::<LittleEndian>()?;
        let mut signatures = Vec::with_capacity(len.into());
        for _ in 0..len {
            signatures.push(FatPointerSignature::zcash_deserialize(&mut reader)?);
        }
        Ok(Self {
            vote_for_block_without_finalizer_public_key: vote,
            signatures,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fat_pointer_round_trip_empty() {
        let fp = FatPointerToBftBlock::null();
        let mut buf = Vec::new();
        fp.zcash_serialize(&mut buf).unwrap();
        let back = FatPointerToBftBlock::zcash_deserialize(&buf[..]).unwrap();
        assert_eq!(fp, back);
        assert!(!back.has_signatures());
    }

    #[test]
    fn v5_header_appends_fat_pointer_v4_does_not() {
        use crate::block::Header;
        use crate::serialization::{ZcashDeserialize, ZcashSerialize};
        use crate::work::equihash::Solution;
        use chrono::TimeZone;

        let mut h = Header {
            version: 4,
            previous_block_hash: crate::block::Hash([0; 32]),
            merkle_root: crate::block::merkle::Root([0; 32]),
            commitment_bytes: [0; 32].into(),
            time: chrono::Utc.timestamp_opt(1, 0).single().unwrap(),
            difficulty_threshold: crate::work::difficulty::CompactDifficulty(0x1d00_ffff),
            nonce: [0; 32].into(),
            solution: Solution::for_proposal(),
            fat_pointer_to_bft_block: FatPointerToBftBlock::null(),
        };
        let v4 = h.zcash_serialize_to_vec().unwrap();
        h.version = 5;
        h.fat_pointer_to_bft_block = FatPointerToBftBlock {
            vote_for_block_without_finalizer_public_key: [3u8; 44],
            signatures: vec![FatPointerSignature {
                public_key: [4u8; 32],
                vote_signature: [5u8; 64],
            }],
        };
        let v5 = h.zcash_serialize_to_vec().unwrap();
        assert!(v5.len() > v4.len(), "v5 must append the fat pointer");
        let back = Header::zcash_deserialize(&v5[..]).unwrap();
        assert_eq!(back.version, 5);
        assert_eq!(back.fat_pointer_to_bft_block, h.fat_pointer_to_bft_block);
    }

    #[test]
    fn fat_pointer_round_trip_one_sig() {
        let fp = FatPointerToBftBlock {
            vote_for_block_without_finalizer_public_key: [7u8; 44],
            signatures: vec![FatPointerSignature {
                public_key: [1u8; 32],
                vote_signature: [2u8; 64],
            }],
        };
        let mut buf = Vec::new();
        fp.zcash_serialize(&mut buf).unwrap();
        let back = FatPointerToBftBlock::zcash_deserialize(&buf[..]).unwrap();
        assert_eq!(fp, back);
        assert_eq!(back.points_at_block_hash(), [7u8; 32]);
    }
}
