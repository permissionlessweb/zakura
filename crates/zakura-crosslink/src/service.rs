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
    /// Local finalizer Ed25519 public key (from `insecure_user_name` / listen identity).
    /// Zebra-crosslink keeps this as `my_public_key` and injects it into the
    /// malachite roster when the peer list omitted us.
    public_key: [u8; 32],
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
        let mut inner = self.inner.lock().await;
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
            TFLServiceRequest::SetFinalBlockHash(hash) => {
                if !inner.activated {
                    return Ok(TFLServiceResponse::SetFinalBlockHash(None));
                }
                if let Some((height, stored)) = inner.latest_final {
                    if stored == hash {
                        return Ok(TFLServiceResponse::SetFinalBlockHash(Some(height)));
                    }
                }
                inner.latest_final = Some((inner.latest_final.map(|p| p.0).unwrap_or(BlockHeight(0)), hash));
                let height = inner.latest_final.map(|p| p.0);
                if let Some(pair) = inner.latest_final {
                    let _ = inner.final_change_tx.send(pair);
                }
                Ok(TFLServiceResponse::SetFinalBlockHash(height))
            }
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
            TFLServiceRequest::TxFinalityStatus(_hash) => {
                let status = match inner.latest_final {
                    None => Some(TFLBlockFinality::NotYetFinalized),
                    Some(_) => Some(TFLBlockFinality::NotYetFinalized),
                };
                Ok(TFLServiceResponse::TxFinalityStatus(status))
            }
            TFLServiceRequest::Roster => {
                // Same malachite workaround as zebra-crosslink: this node's key
                // must appear even if the configured peer roster omitted it.
                if !inner.validators.contains_key(&self.public_key) {
                    inner.validators.insert(self.public_key, 0);
                }
                let roster = inner.validators.iter().map(|(k, v)| (*k, *v)).collect();
                Ok(TFLServiceResponse::Roster(roster))
            }
            TFLServiceRequest::FatPointerToBFTChainTip => {
                Ok(TFLServiceResponse::FatPointerToBFTChainTip(
                    inner.fat_pointer_to_tip.clone(),
                ))
            }
            TFLServiceRequest::StakingCmd(cmd) => {
                drop(inner);
                self.apply_staking_cmd(&cmd).await?;
                Ok(TFLServiceResponse::StakingCmd)
            }
        }
    }

    pub(crate) async fn record_decided(
        &self,
        block: crate::types::BftBlock,
        fp: FatPointerToBftBlock,
    ) {
        let candidate = BlockHeight(block.finalization_candidate_height);
        let cand_hash = BlockHash(block.finalization_candidate().hash);
        let mut inner = self.inner.lock().await;
        inner.bft_blocks.push(block);
        inner.fat_pointer_to_tip = fp;
        inner.latest_final = Some((candidate, cand_hash));
        if let Some(pair) = inner.latest_final {
            let _ = inner.final_change_tx.send(pair);
        }
        info!(pow_final = candidate.0, "tenderlink decided BFT block");
    }

    pub(crate) async fn tip_fat_pointer(&self) -> FatPointerToBftBlock {
        self.inner.lock().await.fat_pointer_to_tip.clone()
    }

    /// Non-blocking snapshot of the current BFT fat pointer.
    pub fn tip_fat_pointer_now(&self) -> FatPointerToBftBlock {
        self.inner
            .try_lock()
            .map(|g| g.fat_pointer_to_tip.clone())
            .unwrap_or_else(|_| FatPointerToBftBlock::null())
    }

    pub(crate) async fn next_bft_height(&self) -> u32 {
        self.inner.lock().await.bft_blocks.len() as u32 + 1
    }

    /// This node's finalizer public key (zebra `my_public_key`).
    pub fn local_public_key(&self) -> [u8; 32] {
        self.public_key
    }

    async fn apply_staking_cmd(&self, cmd: &str) -> Result<(), TFLServiceError> {
        let action = parse_staking_cmd(cmd)?;
        let Some(action) = action else {
            return Ok(());
        };
        let mut inner = self.inner.lock().await;
        apply_staking_action(&mut inner.validators, &action);
        Ok(())
    }
}

fn parse_staking_cmd(
    cmd_str: &str,
) -> Result<Option<zakura_chain::transaction::StakingAction>, TFLServiceError> {
    use zakura_chain::transaction::{StakingAction, StakingActionKind};
    let cmd = cmd_str.as_bytes();
    if cmd.is_empty() {
        return Ok(None);
    }
    if cmd.len() < 4 || cmd[3] != b'|' {
        return Err(TFLServiceError::Misc(format!(
            "Roster command invalid: expected initial instruction\nCMD: \"{cmd_str}\""
        )));
    }
    let kind = match &cmd[..3] {
        b"ADD" => StakingActionKind::CreateNewDelegationBond,
        b"SUB" => StakingActionKind::BeginDelegationUnbonding,
        b"CLR" => StakingActionKind::WithdrawDelegationBond,
        b"MOV" => StakingActionKind::RetargetDelegationBond,
        b"MCL" => StakingActionKind::RetargetDelegationBond,
        _ => {
            return Err(TFLServiceError::Misc(format!(
                "Roster command invalid: unrecognized instruction:\nCMD: \"{cmd_str}\""
            )))
        }
    };
    let rest = &cmd_str[4..];
    let mut parts = rest.split('|');
    let val: u64 = parts
        .next()
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| TFLServiceError::Misc(format!("Roster command invalid: expected u64\nCMD: \"{cmd_str}\"")))?;
    let target_name = parts
        .next()
        .ok_or_else(|| TFLServiceError::Misc(format!("Roster command invalid: expected public address\nCMD: \"{cmd_str}\"")))?
        .to_string();
    let source_name = parts.next().unwrap_or("").to_string();
    let (_, _, target) = rng_keys_from_bytes(target_name.as_bytes());
    let source = if source_name.is_empty() {
        [0u8; 32]
    } else {
        rng_keys_from_bytes(source_name.as_bytes()).2
    };
    if kind == StakingActionKind::RetargetDelegationBond && source_name.is_empty() {
        return Err(TFLServiceError::Misc(format!(
            "Roster command invalid: can't move from non-present finalizer\nCMD: \"{cmd_str}\""
        )));
    }
    let mut action = StakingAction {
        kind,
        amount_zats: val,
        ..StakingAction::default()
    };
    action.arg32_2 = target;
    action.arg32_0 = source;
    let _ = target_name;
    Ok(Some(action))
}

fn apply_staking_action(
    roster: &mut HashMap<[u8; 32], u64>,
    action: &zakura_chain::transaction::StakingAction,
) {
    use zakura_chain::transaction::StakingActionKind;
    let amount = action.amount_zats;
    match action.kind {
        StakingActionKind::CreateNewDelegationBond => {
            *roster.entry(action.arg32_2).or_insert(0) += amount;
        }
        StakingActionKind::BeginDelegationUnbonding | StakingActionKind::WithdrawDelegationBond => {
            let key = action.arg32_0;
            let Some(power) = roster.get_mut(&key) else {
                warn!("staking cmd: subtract target not on roster");
                return;
            };
            *power = power.saturating_sub(amount);
            if *power == 0 {
                roster.remove(&key);
            }
        }
        StakingActionKind::RetargetDelegationBond => {
            let from = action.arg32_0;
            let to = action.arg32_2;
            if let Some(power) = roster.get_mut(&from) {
                *power = power.saturating_sub(amount);
                if *power == 0 {
                    roster.remove(&from);
                }
            }
            *roster.entry(to).or_insert(0) += amount;
        }
        StakingActionKind::Null
        | StakingActionKind::RegisterFinalizer
        | StakingActionKind::ConvertFinalizerRewardToDelegationBond
        | StakingActionKind::UpdateFinalizerKey => {}
    }
}

/// Same derivation zebra-crosslink uses: `DefaultHasher.write(bytes)` then `StdRng`.
pub(crate) fn rng_keys_from_bytes(bytes: &[u8]) -> (u64, SigningKey, [u8; 32]) {
    use rand::{rngs::StdRng, SeedableRng};
    use std::hash::Hasher;
    let mut hasher = std::hash::DefaultHasher::new();
    hasher.write(bytes);
    let seed = hasher.finish();
    let mut rng = StdRng::seed_from_u64(seed);
    let sk = SigningKey::new(&mut rng);
    let vk = VerificationKey::from(&sk);
    (seed, sk, vk.into())
}

pub(crate) fn key_from_name(name: &str) -> (SigningKey, [u8; 32]) {
    let (_, sk, pk) = rng_keys_from_bytes(name.as_bytes());
    (sk, pk)
}

/// Sign the zebra/tenderlink 76-byte vote: `pk ‖ template(44)`.
pub fn sign_fat_pointer(block_hash: &Blake3Hash, keys: &[SigningKey]) -> FatPointerToBftBlock {
    let mut vote = [0u8; 44];
    vote[..32].copy_from_slice(&block_hash.0);
    let signatures = keys
        .iter()
        .map(|sk| {
            let vk = VerificationKey::from(sk);
            let pk: [u8; 32] = vk.into();
            let mut msg = [0u8; 76];
            msg[..32].copy_from_slice(&pk);
            msg[32..].copy_from_slice(&vote);
            let sig: Signature = sk.sign(&msg);
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

/// Reject empty/zero keys and signatures that fail the 76-byte zebra vote.
pub fn reject_bad_fat_pointer(fp: &FatPointerToBftBlock) -> Result<(), TFLServiceError> {
    if !fp.has_signatures() {
        return Err(TFLServiceError::Misc("empty fat pointer signatures".into()));
    }
    if !validate_fat_pointer_signatures(fp) {
        return Err(TFLServiceError::Misc("fat pointer signature verify failed".into()));
    }
    Ok(())
}

fn validate_fat_pointer_signatures(fp: &FatPointerToBftBlock) -> bool {
    if fp.signatures.is_empty() {
        return false;
    }
    let template = &fp.vote_for_block_without_finalizer_public_key;
    for s in &fp.signatures {
        if s.public_key == [0u8; 32] || s.vote_signature == [0u8; 64] {
            return false;
        }
        let Ok(vk) = VerificationKey::try_from(s.public_key) else {
            return false;
        };
        let mut msg = [0u8; 76];
        msg[..32].copy_from_slice(&s.public_key);
        msg[32..].copy_from_slice(template);
        let sig = Signature::from(s.vote_signature);
        if vk.verify(&sig, &msg).is_err() {
            return false;
        }
    }
    true
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
    if config.malachite_peers.is_empty() {
        validators.insert(public_key, 1);
    } else {
        for peer in &config.malachite_peers {
            let (_, _, pk) = rng_keys_from_bytes(peer.as_bytes());
            validators.insert(pk, 1);
        }
        validators.entry(public_key).or_insert(1);
    }

    let params = ZcashCrosslinkParameters {
        bc_confirmation_depth_sigma: config.confirmation_depth_sigma.max(1),
        ..PROTOTYPE_PARAMETERS
    };

    info!(
        public_key = %hex::encode(public_key),
        peers = config.malachite_peers.len(),
        "TFL local finalizer identity"
    );

    let handle = TFLServiceHandle {
        inner: Arc::new(Mutex::new(TFLServiceInternal {
            activated: false,
            activation_height: config.activation_height,
            params,
            signing_key: signing_key.clone(),
            latest_final: None,
            fat_pointer_to_tip: FatPointerToBftBlock::null(),
            bft_blocks: Vec::new(),
            final_change_tx: broadcast::channel(16).0,
            validators,
        })),
        public_key,
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
        let mut inner = handle.inner.lock().await;
        if !inner.activated {
            if tip.0 .0 >= inner.activation_height {
                inner.activated = true;
                info!(height = tip.0 .0, "activating TFL");
            } else {
                return Ok(());
            }
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
    let cand_hash = BlockHash(block.finalization_candidate().hash);

    let mut inner = handle.inner.lock().await;
    inner.bft_blocks.push(block);
    inner.fat_pointer_to_tip = fp;
    inner.latest_final = Some((candidate_height, cand_hash));
    let _ = inner.final_change_tx.send((candidate_height, cand_hash));
    drop(inner);

    let ready = state.ready().await.map_err(|e| e.to_string())?;
    match ready
        .call(StateRequest::CrosslinkFinalizeBlock(cand_hash))
        .await
    {
        Ok(StateResponse::CrosslinkFinalized(hash)) => {
            info!(?hash, "PoW state accepted Crosslink finality");
        }
        Ok(other) => warn!(?other, "unexpected CrosslinkFinalizeBlock response"),
        Err(e) => warn!(?e, "CrosslinkFinalizeBlock failed"),
    }

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
        let (_, _, peer_pk) = rng_keys_from_bytes(id.as_bytes());
        assert_eq!(local_pk, peer_pk);
    }

    #[test]
    fn fat_pointer_signature_public_key_matches_local_identity() {
        let (sk, pk) = key_from_name("zakura-crosslink-lab");
        let fp = sign_fat_pointer(&Blake3Hash([3u8; 32]), &[sk]);
        assert_eq!(fp.signatures.len(), 1);
        assert_eq!(fp.signatures[0].public_key, pk);
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
