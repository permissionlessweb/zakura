//! BFT block + slim PoW header (byte-compatible with terp-rs `crosslink-light-client`).

use std::io::{Read, Write};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use zakura_chain::{
    block::{FatPointerToBftBlock, Header},
    serialization::{SerializationError, ZcashDeserialize, ZcashSerialize},
};

/// Crosslink protocol parameters.
#[derive(Clone, Debug)]
pub struct ZcashCrosslinkParameters {
    /// Best-chain confirmation depth σ.
    pub bc_confirmation_depth_sigma: u64,
    /// Unfinalized gap bound L (unused in the single-node lab loop).
    pub finalization_gap_bound: u64,
}

/// Lab defaults: σ = 3, L = 7.
pub const PROTOTYPE_PARAMETERS: ZcashCrosslinkParameters = ZcashCrosslinkParameters {
    bc_confirmation_depth_sigma: 3,
    finalization_gap_bound: 7,
};

/// 32-byte BLAKE3 hash.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Blake3Hash(pub [u8; 32]);

impl std::fmt::Display for Blake3Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for Blake3Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

/// Slim PoW header carried inside a BFT block (matches terp-rs `PowHeader`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowHeader {
    /// PoW header hash.
    pub hash: [u8; 32],
    /// Unix timestamp.
    pub timestamp: u64,
    /// PoW height.
    pub height: u32,
    /// Raw commitment bytes from the header.
    pub commitment_bytes: [u8; 32],
}

impl PowHeader {
    /// Build from a stock Zakura header plus a known height.
    pub fn from_zakura_header(header: &Header, height: u32) -> Self {
        Self {
            hash: header.hash().0,
            timestamp: u64::try_from(header.time.timestamp()).unwrap_or(0),
            height,
            commitment_bytes: *header.commitment_bytes,
        }
    }
}

impl ZcashSerialize for PowHeader {
    fn zcash_serialize<W: Write>(&self, mut writer: W) -> Result<(), std::io::Error> {
        writer.write_all(&self.hash)?;
        writer.write_u64::<LittleEndian>(self.timestamp)?;
        writer.write_u32::<LittleEndian>(self.height)?;
        writer.write_all(&self.commitment_bytes)?;
        Ok(())
    }
}

impl ZcashDeserialize for PowHeader {
    fn zcash_deserialize<R: Read>(mut reader: R) -> Result<Self, SerializationError> {
        let mut hash = [0u8; 32];
        reader.read_exact(&mut hash)?;
        let timestamp = reader.read_u64::<LittleEndian>()?;
        let height = reader.read_u32::<LittleEndian>()?;
        let mut commitment_bytes = [0u8; 32];
        reader.read_exact(&mut commitment_bytes)?;
        Ok(Self {
            hash,
            timestamp,
            height,
            commitment_bytes,
        })
    }
}

/// BFT block: σ slim PoW headers + previous fat pointer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BftBlock {
    /// Version (1).
    pub version: u32,
    /// BFT height (1-based in this lab).
    pub height: u32,
    /// Fat pointer to the previous BFT block.
    pub previous_block_fat_ptr: FatPointerToBftBlock,
    /// PoW height of the finalization candidate.
    pub finalization_candidate_height: u32,
    /// PoW headers (σ of them).
    pub headers: Vec<PowHeader>,
}

impl BftBlock {
    /// Last header is the finalization candidate.
    pub fn finalization_candidate(&self) -> &PowHeader {
        self.headers
            .last()
            .expect("BftBlock headers should never be empty")
    }

    /// Build after checking confirmation depth.
    pub fn try_from(
        params: &ZcashCrosslinkParameters,
        height: u32,
        previous_block_fat_ptr: FatPointerToBftBlock,
        finalization_candidate_height: u32,
        headers: Vec<PowHeader>,
    ) -> Result<Self, InvalidBftBlock> {
        let expected = params.bc_confirmation_depth_sigma;
        let actual = headers.len() as u64;
        if actual != expected {
            return Err(InvalidBftBlock::IncorrectConfirmationDepth { expected, actual });
        }
        Ok(Self {
            version: 1,
            height,
            previous_block_fat_ptr,
            finalization_candidate_height,
            headers,
        })
    }

    /// Keyed BLAKE3 of the serialized block (tenderlink `HashKeys::value_id`).
    pub fn blake3_hash(&self) -> Blake3Hash {
        let key = tenderlink::HashKeys::default().value_id.0;
        let mut hasher = blake3::Hasher::new_keyed(&key);
        self.zcash_serialize(&mut hasher)
            .expect("hasher write is infallible");
        Blake3Hash(hasher.finalize().into())
    }
}

impl ZcashSerialize for BftBlock {
    fn zcash_serialize<W: Write>(&self, mut writer: W) -> Result<(), std::io::Error> {
        writer.write_u32::<LittleEndian>(self.version)?;
        writer.write_u32::<LittleEndian>(self.height)?;
        self.previous_block_fat_ptr.zcash_serialize(&mut writer)?;
        writer.write_u32::<LittleEndian>(self.finalization_candidate_height)?;
        writer.write_u32::<LittleEndian>(self.headers.len() as u32)?;
        for header in &self.headers {
            header.zcash_serialize(&mut writer)?;
        }
        Ok(())
    }
}

impl ZcashDeserialize for BftBlock {
    fn zcash_deserialize<R: Read>(mut reader: R) -> Result<Self, SerializationError> {
        let version = reader.read_u32::<LittleEndian>()?;
        let height = reader.read_u32::<LittleEndian>()?;
        let previous_block_fat_ptr = FatPointerToBftBlock::zcash_deserialize(&mut reader)?;
        let finalization_candidate_height = reader.read_u32::<LittleEndian>()?;
        let header_count = reader.read_u32::<LittleEndian>()?;
        if header_count > 2048 {
            return Err(SerializationError::Parse(
                "header_count was greater than 2048.",
            ));
        }
        let mut headers = Vec::with_capacity(header_count as usize);
        for _ in 0..header_count {
            headers.push(PowHeader::zcash_deserialize(&mut reader)?);
        }
        Ok(Self {
            version,
            height,
            previous_block_fat_ptr,
            finalization_candidate_height,
            headers,
        })
    }
}

/// Validation error for [`BftBlock`].
#[derive(Debug, Error)]
pub enum InvalidBftBlock {
    /// Wrong number of headers vs σ.
    #[error("invalid confirmation depth: expected {expected}, got {actual}")]
    IncorrectConfirmationDepth {
        /// Expected σ.
        expected: u64,
        /// Actual header count.
        actual: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bft_block_round_trip() {
        let params = PROTOTYPE_PARAMETERS;
        let headers = (0..3)
            .map(|i| PowHeader {
                hash: [i; 32],
                timestamp: 1_700_000_000 + i as u64,
                height: 10 + u32::from(i),
                commitment_bytes: [i ^ 0x5a; 32],
            })
            .collect();
        let block = BftBlock::try_from(&params, 1, FatPointerToBftBlock::null(), 12, headers)
            .expect("sigma=3");
        let mut buf = Vec::new();
        block.zcash_serialize(&mut buf).unwrap();
        let back = BftBlock::zcash_deserialize(&buf[..]).unwrap();
        assert_eq!(block, back);
        assert_ne!(block.blake3_hash().0, [0u8; 32]);
    }
}
