//! TFL / Crosslink request and response types.
//!
//! Matches `zebra-state/src/crosslink.rs` from ShieldedLabs/zebra-crosslink so
//! the TFL service and JSON-RPC surface stay name-compatible.

use std::fmt;

use tokio::sync::broadcast;

use zakura_chain::block::{FatPointerToBftBlock, Hash as BlockHash, Height as BlockHeight};
use zakura_chain::transaction::Hash as TxHash;

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
    /// Force a final-block hash (lab / `set_tfl_finality_by_hash`).
    SetFinalBlockHash(BlockHash),
    /// Get the finality status of a block.
    BlockFinalityStatus(BlockHeight, BlockHash),
    /// Get the finality status of a transaction.
    TxFinalityStatus(TxHash),
    /// Get the finalizer roster (pubkey, voting power).
    Roster,
    /// Get the fat pointer to the BFT chain tip.
    FatPointerToBFTChainTip,
    /// Submit a staking command string (`ADD|val|name`, …).
    StakingCmd(String),
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
    /// Height after a forced finality set, if the hash was found.
    SetFinalBlockHash(Option<BlockHeight>),
    /// Finality of one block.
    BlockFinalityStatus(Option<TFLBlockFinality>),
    /// Finality of one transaction.
    TxFinalityStatus(Option<TFLBlockFinality>),
    /// Roster entries.
    Roster(Vec<([u8; 32], u64)>),
    /// Fat pointer to the BFT tip (typed, zebra-crosslink shape).
    FatPointerToBFTChainTip(FatPointerToBftBlock),
    /// Staking command accepted.
    StakingCmd,
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
