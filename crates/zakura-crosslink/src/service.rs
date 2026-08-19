//! Single-node TFL service (lab prototype).
//!
//! Multi-node tenderlink/malachite from zebra-crosslink is not required for
//! local ict-rs / terp-rs LC tests. This node is the sole finalizer: it
//! proposes a BFT block once the PoW tip is σ above the last final, signs
//! the fat pointer with a deterministic Ed25519 key, and exposes it on RPC.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use ed25519_zebra::{Signature, SigningKey, VerificationKey};
use tokio::sync::{broadcast, Mutex};
use tower::{Service, ServiceExt};
use tracing::{debug, info, warn};
use zakura_chain::block::{
    FatPointerSignature, FatPointerToBftBlock, Hash as BlockHash, Height as BlockHeight,
};
use zakura_chain::serialization::ZcashSerialize;
use zakura_state::crosslink::{
    TFLBlockFinality, TFLServiceError, TFLServiceRequest, TFLServiceResponse,
};
use zakura_state::{Request as StateRequest, Response as StateResponse};

use crate::config::Config;
use crate::types::{
    BftBlock, Blake3Hash, PowHeader, ZcashCrosslinkParameters, PROTOTYPE_PARAMETERS,
};

/// Process-wide handle so RPC can call TFL without plumbing generics through RpcImpl.
static GLOBAL: OnceLock<TFLServiceHandle> = OnceLock::new();

/// Install the process-wide TFL handle (once).
pub fn install_global(handle: TFLServiceHandle) {
    let _ = GLOBAL.set(handle);
}

/// Borrow the process-wide handle, if TFL was spawned.
pub fn global() -> Option<TFLServiceHandle> {
    GLOBAL.get().cloned()
}

/// Shared TFL handle.
#[derive(Clone)]
pub struct TFLServiceHandle {
    inner: Arc<Mutex<TFLServiceInternal>>,
}

impl std::fmt::Debug for TFLServiceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TFLServiceHandle").finish_non_exhaustive()
    }
}

struct TFLServiceInternal {
    activated: bool,
    activation_height: u32,
    params: ZcashCrosslinkParameters,
    signing_key: SigningKey,
    public_key: [u8; 32],
    latest_final: Option<(BlockHeight, BlockHash)>,
    fat_pointer_to_tip: FatPointerToBftBlock,
    bft_blocks: Vec<BftBlock>,
    final_change_tx: broadcast::Sender<(BlockHeight, BlockHash)>,
    validators: HashMap<[u8; 32], u64>,
}

impl TFLServiceHandle {
    /// Handle one TFL request.
    pub async fn call(
        &self,
        request: TFLServiceRequest,
    ) -> Result<TFLServiceResponse, TFLServiceError> {
        let inner = self.inner.lock().await;
        match request {
            TFLServiceRequest::IsTFLActivated => {
                Ok(TFLServiceResponse::IsTFLActivated(inner.activated))
            }
            TFLServiceRequest::FinalBlockHeightHash => {
                Ok(TFLServiceResponse::FinalBlockHeightHash(inner.latest_final))
            }
            TFLServiceRequest::FinalBlockRx => Ok(TFLServiceResponse::FinalBlockRx(
                inner.final_change_tx.subscribe(),
            )),
            TFLServiceRequest::BlockFinalityStatus(height, hash) => {
                let status = match inner.latest_final {
                    None => Some(TFLBlockFinality::NotYetFinalized),
                    Some((fh, _)) if height > fh => Some(TFLBlockFinality::NotYetFinalized),
                    Some((fh, fhash)) if height == fh && hash == fhash => {
                        Some(TFLBlockFinality::Finalized)
                    }
                    Some((fh, _)) if height == fh => Some(TFLBlockFinality::CantBeFinalized),
                    Some(_) => Some(TFLBlockFinality::Finalized),
                };
                Ok(TFLServiceResponse::BlockFinalityStatus(status))
            }
            TFLServiceRequest::Roster => {
                let roster = inner.validators.iter().map(|(k, v)| (*k, *v)).collect();
                Ok(TFLServiceResponse::Roster(roster))
            }
            TFLServiceRequest::FatPointerToBFTChainTip => {
                let mut buf = Vec::new();
                inner
                    .fat_pointer_to_tip
                    .zcash_serialize(&mut buf)
                    .map_err(|e| TFLServiceError::Misc(e.to_string()))?;
                Ok(TFLServiceResponse::FatPointerToBFTChainTip(buf))
            }
        }
    }

    pub(crate) async fn record_decided(
        &self,
        block: crate::types::BftBlock,
        fp: FatPointerToBftBlock,
    ) {
        let candidate = BlockHeight(block.finalization_candidate_height);
        let mut inner = self.inner.lock().await;
        inner.bft_blocks.push(block);
        inner.fat_pointer_to_tip = fp;
        if let Some((_, hash)) = inner.latest_final {
            inner.latest_final = Some((candidate, hash));
        } else {
            inner.latest_final = Some((candidate, BlockHash([0u8; 32])));
        }
        if let Some(pair) = inner.latest_final {
            let _ = inner.final_change_tx.send(pair);
        }
        info!(pow_final = candidate.0, "tenderlink decided BFT block");
    }

    pub(crate) async fn tip_fat_pointer(&self) -> FatPointerToBftBlock {
        self.inner.lock().await.fat_pointer_to_tip.clone()
    }

    pub(crate) async fn next_bft_height(&self) -> u32 {
        self.inner.lock().await.bft_blocks.len() as u32 + 1
    }
}

pub(crate) fn rng_keys_from_bytes(bytes: &[u8]) -> (u64, SigningKey, [u8; 32]) {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let seed = hasher.finish();
    let mut arr = [0u8; 32];
    arr[..8].copy_from_slice(&seed.to_le_bytes());
    let sk = SigningKey::from(arr);
    let vk = VerificationKey::from(&sk);
    (seed, sk, vk.into())
}

pub(crate) fn key_from_name(name: &str) -> (SigningKey, [u8; 32]) {
    let mut seed = [0u8; 32];
    let bytes = name.as_bytes();
    let n = bytes.len().min(32);
    seed[..n].copy_from_slice(&bytes[..n]);
    let sk = SigningKey::from(seed);
    let vk = VerificationKey::from(&sk);
    (sk, vk.into())
}

/// Sign a 44-byte vote (block hash || 12 zero bytes).
pub fn sign_fat_pointer(block_hash: &Blake3Hash, keys: &[SigningKey]) -> FatPointerToBftBlock {
    let mut vote = [0u8; 44];
    vote[..32].copy_from_slice(&block_hash.0);
    let signatures = keys
        .iter()
        .map(|sk| {
            let vk = VerificationKey::from(sk);
            let pk: [u8; 32] = vk.into();
            let sig: Signature = sk.sign(&vote);
            FatPointerSignature {
                public_key: pk,
                vote_signature: sig.to_bytes(),
            }
        })
        .collect();
    FatPointerToBftBlock {
        vote_for_block_without_finalizer_public_key: vote,
        signatures,
    }
}

/// Reject null / empty / all-zero-key fat pointers (CX5).
pub fn reject_bad_fat_pointer(fp: &FatPointerToBftBlock) -> Result<(), TFLServiceError> {
    if !fp.has_signatures() {
        return Err(TFLServiceError::Misc("empty fat pointer signatures".into()));
    }
    if fp.signatures.iter().any(|s| s.public_key == [0u8; 32]) {
        return Err(TFLServiceError::Misc("zero public key".into()));
    }
    if fp.signatures.iter().any(|s| s.vote_signature == [0u8; 64]) {
        return Err(TFLServiceError::Misc("zero signature".into()));
    }
    Ok(())
}

/// Spawn the TFL main loop. `state` is the node state service.
pub fn spawn_tfl_service<S>(state: S, config: Config) -> TFLServiceHandle
where
    S: Service<StateRequest, Response = StateResponse, Error = zakura_state::BoxError>
        + Clone
        + Send
        + Sync
        + 'static,
    S::Future: Send,
{
    let name = config
        .insecure_user_name
        .clone()
        .unwrap_or_else(|| "zakura-crosslink-lab".into());
    let (signing_key, public_key) = key_from_name(&name);
    let mut validators = HashMap::new();
    validators.insert(public_key, 1);

    let params = ZcashCrosslinkParameters {
        bc_confirmation_depth_sigma: config.confirmation_depth_sigma.max(1),
        ..PROTOTYPE_PARAMETERS
    };

    let handle = TFLServiceHandle {
        inner: Arc::new(Mutex::new(TFLServiceInternal {
            activated: true,
            activation_height: config.activation_height,
            params,
            signing_key: signing_key.clone(),
            public_key,
            latest_final: None,
            fat_pointer_to_tip: FatPointerToBftBlock::null(),
            bft_blocks: Vec::new(),
            final_change_tx: broadcast::channel(16).0,
            validators,
        })),
    };

    if config.tenderlink_enabled() {
        crate::tenderlink::spawn_tenderlink(handle.clone(), state, config, signing_key);
    } else {
        let loop_handle = handle.clone();
        tokio::spawn(async move { tfl_main_loop(loop_handle, state).await });
    }
    handle
}

async fn tfl_main_loop<S>(handle: TFLServiceHandle, mut state: S)
where
    S: Service<StateRequest, Response = StateResponse, Error = zakura_state::BoxError>
        + Clone
        + Send
        + 'static,
    S::Future: Send,
{
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    loop {
        interval.tick().await;
        if let Err(e) = tick_once(&handle, &mut state).await {
            debug!(?e, "tfl tick");
        }
    }
}

async fn tick_once<S>(handle: &TFLServiceHandle, state: &mut S) -> Result<(), String>
where
    S: Service<StateRequest, Response = StateResponse, Error = zakura_state::BoxError> + Clone,
    S::Future: Send,
{
    let ready = state.ready().await.map_err(|e| e.to_string())?;
    let tip = match ready
        .call(StateRequest::Tip)
        .await
        .map_err(|e| e.to_string())?
    {
        StateResponse::Tip(Some((h, hash))) => (h, hash),
        _ => return Ok(()),
    };

    let (activation, sigma, last_final, sk) = {
        let inner = handle.inner.lock().await;
        if !inner.activated {
            return Ok(());
        }
        (
            inner.activation_height,
            inner.params.bc_confirmation_depth_sigma,
            inner.latest_final,
            inner.signing_key,
        )
    };

    if tip.0 .0 < activation.saturating_add(sigma as u32) {
        return Ok(());
    }

    let candidate_height = BlockHeight(tip.0 .0.saturating_sub(sigma as u32));
    if let Some((fh, _)) = last_final {
        if candidate_height <= fh {
            return Ok(());
        }
    }

    let mut headers = Vec::new();
    for offset in 0..sigma {
        let h = BlockHeight(
            candidate_height
                .0
                .saturating_sub((sigma - 1 - offset) as u32),
        );
        let ready = state.ready().await.map_err(|e| e.to_string())?;
        let resp = ready
            .call(StateRequest::BlockHeader(h.into()))
            .await
            .map_err(|e| e.to_string())?;
        match resp {
            StateResponse::BlockHeader { header, .. } => {
                headers.push(PowHeader::from_zakura_header(&header, h.0));
            }
            other => {
                warn!(?other, "unexpected BlockHeader response");
                return Ok(());
            }
        }
    }

    let prev_fp = handle.inner.lock().await.fat_pointer_to_tip.clone();
    let bft_height = handle.inner.lock().await.bft_blocks.len() as u32 + 1;
    let block = BftBlock::try_from(
        &ZcashCrosslinkParameters {
            bc_confirmation_depth_sigma: sigma,
            finalization_gap_bound: 7,
        },
        bft_height,
        prev_fp,
        candidate_height.0,
        headers,
    )
    .map_err(|e| e.to_string())?;

    let hash = block.blake3_hash();
    let fp = sign_fat_pointer(&hash, &[sk]);
    reject_bad_fat_pointer(&fp).map_err(|e| e.to_string())?;

    let mut inner = handle.inner.lock().await;
    inner.bft_blocks.push(block);
    inner.fat_pointer_to_tip = fp;
    inner.latest_final = Some((candidate_height, tip.1));
    let _ = inner.final_change_tx.send((candidate_height, tip.1));
    info!(
        bft_height,
        pow_final = candidate_height.0,
        "crosslink TFL finalized"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_roster_key_matches_insecure_user_name() {
        let id = "pow-a:24834";
        let (_, local_pk) = key_from_name(id);
        let (_, peer_pk) = key_from_name(id);
        assert_eq!(local_pk, peer_pk);
        let (_, _, old_wrong) = rng_keys_from_bytes(id.as_bytes());
        assert_ne!(
            local_pk, old_wrong,
            "rng_keys_from_bytes must not be used for roster peers"
        );
    }

    #[test]
    fn rejects_null_and_accepts_signed() {
        assert!(reject_bad_fat_pointer(&FatPointerToBftBlock::null()).is_err());
        let hash = Blake3Hash([9u8; 32]);
        let (sk, _) = key_from_name("lab");
        let good = sign_fat_pointer(&hash, &[sk]);
        assert!(reject_bad_fat_pointer(&good).is_ok());
        assert!(good.has_signatures());
    }
}
