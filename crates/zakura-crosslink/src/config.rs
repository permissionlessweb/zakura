//! Crosslink / TFL configuration. Field names match zebra-crosslink so a
//! Shielded Labs testnet toml can be reused with only crate-prefix changes.

use serde::{Deserialize, Serialize};

/// `[crosslink]` section in `zakura.toml`.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// When false, TFL is not spawned (stock PoW node).
    pub enabled: bool,
    /// PoW height at which TFL activates. Regtest default is 3.
    pub activation_height: u32,
    /// Confirmation depth σ (headers embedded in each BFT block).
    pub confirmation_depth_sigma: u64,
    /// Deterministic seed for the local finalizer key (lab / testnet identity).
    pub insecure_user_name: Option<String>,
    /// Local tenderlink listen, e.g. `0.0.0.0:24834` (IP:port, not multiaddr).
    pub listen_address: Option<String>,
    /// Advertised public IP:port for this finalizer.
    pub public_address: Option<String>,
    /// Other finalizers on the Crosslink testnet, same `IP:port` form.
    pub malachite_peers: Vec<String>,
    /// Passed through to zebra-crosslink-compatible configs (unused here).
    pub do_not_manipulate_config: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            activation_height: 3,
            confirmation_depth_sigma: 3,
            insecure_user_name: Some("zakura-crosslink-lab".to_string()),
            listen_address: None,
            public_address: None,
            malachite_peers: Vec::new(),
            do_not_manipulate_config: false,
        }
    }
}

impl Config {
    /// Tenderlink is required when we must talk to other finalizers.
    pub fn tenderlink_enabled(&self) -> bool {
        self.enabled
            && (self.listen_address.is_some()
                || self.public_address.is_some()
                || !self.malachite_peers.is_empty())
    }
}
