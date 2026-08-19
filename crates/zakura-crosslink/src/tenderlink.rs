//! Tenderlink (Shielded Labs / malachite-style) BFT networking.
//!
//! This is the protocol the current Crosslink prototype testnet speaks.
//! Single-node auto-sign remains available when `[crosslink]` has no peers.

use std::hash::{DefaultHasher, Hasher};
use std::net::SocketAddr;
use std::sync::Arc;

use ed25519_zebra::SigningKey;
use tenderlink::{
    BlockValue, ClosureToGetHistoricalBlock, ClosureToProposeNewBlock, ClosureToPushDecidedBlock,
    ClosureToUpdateRosterCmd, ClosureToValidateProposedBlock, EndpointEvidence,
    FatPointerToBftBlock3, PubKeyID, SecureUdpEndpoint, SortedRosterMember, StaticDHKeyPair,
    TMStatus,
};
use tower::{Service, ServiceExt};
use tracing::{error, info, warn};
use zakura_chain::block::{
    FatPointerSignature, FatPointerToBftBlock, Header, Height as BlockHeight,
};
use zakura_chain::serialization::{ZcashDeserialize, ZcashSerialize};
use zakura_state::{Request as StateRequest, Response as StateResponse};

use crate::config::Config;
use crate::service::{reject_bad_fat_pointer, TFLServiceHandle};
use crate::types::{BftBlock, PowHeader};

/// BFT block on the zebra-crosslink wire (full PoW headers).
#[derive(Clone, Debug)]
pub struct WireBftBlock {
    pub version: u32,
    pub height: u32,
    pub previous_block_fat_ptr: FatPointerToBftBlock,
    pub finalization_candidate_height: u32,
    pub headers: Vec<Header>,
}

impl ZcashSerialize for WireBftBlock {
    fn zcash_serialize<W: std::io::Write>(&self, mut writer: W) -> Result<(), std::io::Error> {
        use byteorder::{LittleEndian, WriteBytesExt};
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

impl ZcashDeserialize for WireBftBlock {
    fn zcash_deserialize<R: std::io::Read>(
        mut reader: R,
    ) -> Result<Self, zakura_chain::serialization::SerializationError> {
        use byteorder::{LittleEndian, ReadBytesExt};
        use zakura_chain::serialization::SerializationError;
        let version = reader.read_u32::<LittleEndian>()?;
        let height = reader.read_u32::<LittleEndian>()?;
        let previous_block_fat_ptr = FatPointerToBftBlock::zcash_deserialize(&mut reader)?;
        let finalization_candidate_height = reader.read_u32::<LittleEndian>()?;
        let header_count = reader.read_u32::<LittleEndian>()?;
        if header_count > 2048 {
            return Err(SerializationError::Parse("header_count > 2048"));
        }
        let mut headers = Vec::with_capacity(header_count as usize);
        for _ in 0..header_count {
            headers.push(Header::zcash_deserialize(&mut reader)?);
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

impl WireBftBlock {
    fn to_slim(&self) -> BftBlock {
        let headers = self
            .headers
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let height = self
                    .finalization_candidate_height
                    .saturating_sub((self.headers.len().saturating_sub(1 + i)) as u32);
                PowHeader::from_zakura_header(h, height)
            })
            .collect();
        BftBlock {
            version: self.version,
            height: self.height,
            previous_block_fat_ptr: self.previous_block_fat_ptr.clone(),
            finalization_candidate_height: self.finalization_candidate_height,
            headers,
        }
    }
}

fn fp3_to_ours(fp: FatPointerToBftBlock3) -> FatPointerToBftBlock {
    FatPointerToBftBlock {
        vote_for_block_without_finalizer_public_key: fp.vote_for_block_without_finalizer_public_key,
        signatures: fp
            .signatures
            .into_iter()
            .map(|s| FatPointerSignature {
                public_key: s.public_key,
                vote_signature: s.vote_signature,
            })
            .collect(),
    }
}

fn parse_ip_port(s: &str) -> Result<([u8; 16], u16), String> {
    use std::net::ToSocketAddrs;
    let sa: SocketAddr = s
        .parse()
        .ok()
        .or_else(|| s.to_socket_addrs().ok().and_then(|mut i| i.next()))
        .ok_or_else(|| format!("bad addr {s}"))?;
    let (ip6, port) = match sa {
        SocketAddr::V4(v4) => (v4.ip().to_ipv6_mapped(), v4.port()),
        SocketAddr::V6(v6) => (*v6.ip(), v6.port()),
    };
    Ok((ip6.octets(), port))
}

fn parse_ip_port_retry(s: &str, attempts: u32) -> Result<([u8; 16], u16), String> {
    let mut last = "no attempt".to_string();
    for i in 0..attempts.max(1) {
        match parse_ip_port(s) {
            Ok(v) => return Ok(v),
            Err(e) => {
                last = e;
                if i + 1 < attempts {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            }
        }
    }
    Err(last)
}

/// Noise identity is keyed by `identity` (public roster name). Bind/connect
/// address is `endpoint_addr` so `0.0.0.0:24834` can listen while peers
/// derive the same key from `pow-a:24834`.
fn addr_to_noise(identity: &str, endpoint_addr: &str) -> (StaticDHKeyPair, SecureUdpEndpoint) {
    let mut hasher = DefaultHasher::new();
    hasher.write(identity.as_bytes());
    let seed = hasher.finish();
    let kp = snow::Builder::with_resolver(
        "Noise_IK_25519_ChaChaPoly_BLAKE2s".parse().unwrap(),
        Box::new(tenderlink::SnowRngResolver::seed_from_u64(seed)),
    )
    .generate_keypair()
    .unwrap();
    let static_keypair = StaticDHKeyPair {
        private: kp.private.try_into().unwrap(),
        public: kp.public.try_into().unwrap(),
    };
    let (ip, port) = parse_ip_port_retry(endpoint_addr, 80).unwrap_or(([0u8; 16], 24834));
    (
        static_keypair,
        SecureUdpEndpoint {
            public_key: static_keypair.public,
            ip_address: ip,
            port,
        },
    )
}

/// Spawn tenderlink so this node can sit on the Crosslink prototype testnet.
pub fn spawn_tenderlink<S>(
    handle: TFLServiceHandle,
    mut state: S,
    config: Config,
    signing_key: SigningKey,
) where
    S: Service<StateRequest, Response = StateResponse, Error = zakura_state::BoxError>
        + Clone
        + Send
        + Sync
        + 'static,
    S::Future: Send,
{
    let public_ip = config
        .public_address
        .clone()
        .or_else(|| config.listen_address.clone())
        .unwrap_or_else(|| "127.0.0.1:24834".into());
    let user_name = config
        .insecure_user_name
        .clone()
        .unwrap_or_else(|| public_ip.clone());

    let identity = config
        .insecure_user_name
        .clone()
        .unwrap_or_else(|| public_ip.clone());

    let mut static_keypair_maybe = None;
    let mut endpoint_maybe = None;
    if let Some(listen_addr) = &config.listen_address {
        // Key from advertised identity; bind address from listen_address.
        let (a, b) = addr_to_noise(&identity, listen_addr);
        static_keypair_maybe = Some(a);
        endpoint_maybe = Some(b);
    }

    // Roster: ourselves + configured peers. Peer keys MUST use the same
    // `key_from_name` derivation as `spawn_tfl_service` uses for
    // `insecure_user_name` (set that to this node's `public_address`).
    let mut roster: Vec<SortedRosterMember> = Vec::new();
    let mut evidence: Vec<EndpointEvidence> = Vec::new();

    let my_pk: [u8; 32] = {
        let vk = ed25519_zebra::VerificationKey::from(&signing_key);
        vk.into()
    };
    roster.push(SortedRosterMember {
        pub_key: PubKeyID(my_pk),
        stake: 1,
        cumulative_stake: 1,
    });

    for peer in config.malachite_peers.iter() {
        let (_, pk) = crate::service::key_from_name(peer);
        roster.push(SortedRosterMember {
            pub_key: PubKeyID(pk),
            stake: 1,
            cumulative_stake: 0,
        });
        let (_kp, ep) = addr_to_noise(peer, peer);
        evidence.push(EndpointEvidence {
            endpoint: ep,
            root_public_key: pk,
        });
        let _ = user_name;
    }

    // Every node must share the same roster order or proposer selection forks.
    // Match zebra-crosslink: descending (stake, pub_key), then prefix-sum stake.
    roster.sort_by_key(|m| std::cmp::Reverse((m.stake, m.pub_key.0)));
    let mut cumulative = 0u64;
    for m in &mut roster {
        cumulative = cumulative.saturating_add(m.stake);
        m.cumulative_stake = cumulative;
    }
    info!(
        roster_n = roster.len(),
        first = ?roster.first().map(|m| hex::encode(m.pub_key.0)),
        "tenderlink roster sorted"
    );

    let h_propose = handle.clone();
    let h_validate = handle.clone();
    let h_decide = handle.clone();
    let mut state_p = state.clone();
    let sigma = config.confirmation_depth_sigma.max(1);
    let activation = config.activation_height;

    info!(
        peers = config.malachite_peers.len(),
        listen = ?config.listen_address,
        "starting tenderlink (Crosslink testnet BFT)"
    );

    tokio::spawn(async move {
        let result = tenderlink::entry_point(
            signing_key,
            static_keypair_maybe,
            endpoint_maybe,
            roster,
            evidence,
            None,
            ClosureToProposeNewBlock(Arc::new(move || {
                let handle = h_propose.clone();
                let mut state = state_p.clone();
                Box::pin(async move {
                    match propose_wire(&handle, &mut state, activation, sigma).await {
                        Ok(Some(block)) => {
                            let mut buf = Vec::new();
                            block
                                .zcash_serialize(&mut buf)
                                .ok()
                                .map(|_| BlockValue(buf))
                        }
                        _ => None,
                    }
                })
            })),
            ClosureToValidateProposedBlock(Arc::new(move |block| {
                let handle = h_validate.clone();
                Box::pin(async move {
                    match WireBftBlock::zcash_deserialize(block.0.as_slice()) {
                        Ok(wire) => {
                            if wire.headers.is_empty() {
                                return TMStatus::Fail;
                            }
                            let tip_fp = handle.tip_fat_pointer().await;
                            if wire.previous_block_fat_ptr.points_at_block_hash()
                                != tip_fp.points_at_block_hash()
                            {
                                warn!("tenderlink: prev fat pointer mismatch");
                                return TMStatus::Fail;
                            }
                            TMStatus::Pass
                        }
                        Err(e) => {
                            warn!(?e, "tenderlink: failed to deserialize proposed BFT block");
                            TMStatus::Fail
                        }
                    }
                })
            })),
            ClosureToPushDecidedBlock(Arc::new(move |block, fat3| {
                let handle = h_decide.clone();
                Box::pin(async move {
                    if let Ok(wire) = WireBftBlock::zcash_deserialize(block.0.as_slice()) {
                        let slim = wire.to_slim();
                        let fp = fp3_to_ours(fat3);
                        if reject_bad_fat_pointer(&fp).is_ok() {
                            handle.record_decided(slim, fp).await;
                        } else {
                            error!("tenderlink decided block has invalid fat pointer");
                        }
                    }
                    Vec::new()
                })
            })),
            ClosureToGetHistoricalBlock(Arc::new(move |_height| {
                Box::pin(async move {
                    panic!("tenderlink historical block fetch not implemented");
                })
            })),
            ClosureToUpdateRosterCmd(Arc::new(move |s| Box::pin(async move { s }))),
        )
        .await;
        if let Err(e) = result {
            error!(?e, "tenderlink entry_point exited");
        }
    });
}

async fn propose_wire<S>(
    handle: &TFLServiceHandle,
    state: &mut S,
    activation: u32,
    sigma: u64,
) -> Result<Option<WireBftBlock>, String>
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
        StateResponse::Tip(Some((h, _))) => h,
        _ => return Ok(None),
    };
    if tip.0 < activation.saturating_add(sigma as u32) {
        return Ok(None);
    }
    let candidate = BlockHeight(tip.0.saturating_sub(sigma as u32));
    let mut headers = Vec::new();
    for offset in 0..sigma {
        let h = BlockHeight(candidate.0.saturating_sub((sigma - 1 - offset) as u32));
        let ready = state.ready().await.map_err(|e| e.to_string())?;
        match ready
            .call(StateRequest::BlockHeader(h.into()))
            .await
            .map_err(|e| e.to_string())?
        {
            StateResponse::BlockHeader { header, .. } => {
                headers.push(std::sync::Arc::unwrap_or_clone(header))
            }
            _ => return Ok(None),
        }
    }
    let prev = handle.tip_fat_pointer().await;
    let height = handle.next_bft_height().await;
    Ok(Some(WireBftBlock {
        version: 1,
        height,
        previous_block_fat_ptr: prev,
        finalization_candidate_height: candidate.0,
        headers,
    }))
}
