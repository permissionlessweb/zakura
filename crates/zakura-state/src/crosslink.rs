//! TFL / Crosslink request and response types (lab prototype).
//!
//! PoW state does not need to store finality for this prototype: the TFL
//! service in `zakura-crosslink` tracks finalized height in-process.

use std::fmt;

use tokio::sync::broadcast;

use zakura_chain::block::{Hash as BlockHash, Height as BlockHeight};

/// The finality status of a block.
#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum TFLBlockFinality {
    /// Height is above the finalized height.
    NotYetFinalized,
    /// Height is at or below finalized height and the hash is the best-chain block.
    Finalized,
    /// Height is at or below finalized height but the hash is not on the best chain.
    CantBeFinalized,
}

/// Requests to the TFL service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TFLServiceRequest {
    /// Is the TFL service activated yet?
    IsTFLActivated,
    /// Get the final block height and hash.
    FinalBlockHeightHash,
    /// Subscribe to final-block changes.
    FinalBlockRx,
    /// Get the finality status of a block.
    BlockFinalityStatus(BlockHeight, BlockHash),
    /// Get the finalizer roster (pubkey, voting power).
    Roster,
    /// Get the fat pointer to the BFT chain tip.
    FatPointerToBFTChainTip,
}

/// Responses from the TFL service.
#[derive(Debug)]
pub enum TFLServiceResponse {
    /// Activation flag.
    IsTFLActivated(bool),
    /// Final block, if any.
    FinalBlockHeightHash(Option<(BlockHeight, BlockHash)>),
    /// Subscriber for final-block changes.
    FinalBlockRx(broadcast::Receiver<(BlockHeight, BlockHash)>),
    /// Finality of one block.
    BlockFinalityStatus(Option<TFLBlockFinality>),
    /// Roster entries.
    Roster(Vec<([u8; 32], u64)>),
    /// Fat pointer bytes (ZcashSerialize of [`zakura_chain::block::FatPointerToBftBlock`]).
    FatPointerToBFTChainTip(Vec<u8>),
}

/// Errors from the TFL service.
#[derive(Debug)]
pub enum TFLServiceError {
    /// Not implemented.
    NotImplemented,
    /// Other error.
    Misc(String),
}

impl fmt::Display for TFLServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TFLServiceError: {self:?}")
    }
}

impl std::error::Error for TFLServiceError {}
