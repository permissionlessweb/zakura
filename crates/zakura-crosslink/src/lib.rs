//! Zakura Trailing Finality Layer (Crosslink) — lab prototype.
//!
//! Feature-off `zakurad` does not link this crate. Feature-on + `[crosslink]
//! enabled = true` spawns a single-node TFL gadget that signs fat pointers
//! with a deterministic Ed25519 key and exposes zebra-crosslink-compatible
//! JSON-RPC names.

pub mod config;
pub mod service;
pub mod tenderlink;
pub mod types;

pub use config::Config;
pub use service::{
    global, install_global, reject_bad_fat_pointer, sign_fat_pointer, spawn_tfl_service,
    TFLServiceHandle,
};
pub use types::{
    BftBlock, Blake3Hash, InvalidBftBlock, PowHeader, ZcashCrosslinkParameters, PROTOTYPE_PARAMETERS,
};
pub use zakura_chain::block::{FatPointerSignature, FatPointerToBftBlock};
pub use zakura_state::crosslink::{
    TFLBlockFinality, TFLServiceError, TFLServiceRequest, TFLServiceResponse,
};
