//! Delegation bond serialization (Season 1 / crosslink_monolith v13).
//!
//! Adding these column families is a minor format change.

use zakura_chain::amount::{Amount, NonNegative};

use crate::service::finalized_state::disk_format::{
    block::TransactionLocation, FromDisk, IntoDisk,
};

/// Unique pubkey from staking `arg32_0`.
pub type BondKey = [u8; 32];

/// A delegation bond stored in state.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct DelegationBond {
    /// Bond principal plus accrued POS rewards, in zatoshis.
    pub amount: Amount<NonNegative>,
    /// Target finalizer (`arg32_2` at create, updated on retarget).
    pub target_finalizer: [u8; 32],
    /// Location of the creating (or last status-changing) transaction.
    pub created_at: TransactionLocation,
}

impl DelegationBond {
    /// New bond row.
    pub fn new(
        amount: Amount<NonNegative>,
        target_finalizer: [u8; 32],
        created_at: TransactionLocation,
    ) -> Self {
        Self {
            amount,
            target_finalizer,
            created_at,
        }
    }
}

/// Status of a finalized delegation bond.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum BondStatus {
    /// Bond is active and can be unbonded.
    Active,
    /// `BeginDelegationUnbonding` has been applied.
    Unbonding {
        /// Location of the unbond transaction.
        unbonded_at: TransactionLocation,
    },
    /// `WithdrawDelegationBond` has been applied.
    Withdrawn {
        /// Location of the withdraw transaction.
        withdrawn_at: TransactionLocation,
    },
    /// Burned by a social-slashing fork (not applied until slash rules land).
    Burned,
}

impl BondStatus {
    /// True if the bond is active.
    pub fn is_active(&self) -> bool {
        matches!(self, BondStatus::Active)
    }
}

const BOND_KEY_DISK_BYTES: usize = 32;
const TARGET_FINALIZER_DISK_BYTES: usize = 32;
const AMOUNT_DISK_BYTES: usize = 8;
const TRANSACTION_LOCATION_DISK_BYTES: usize = 5;
const DELEGATION_BOND_DISK_BYTES: usize =
    AMOUNT_DISK_BYTES + TARGET_FINALIZER_DISK_BYTES + TRANSACTION_LOCATION_DISK_BYTES;

impl IntoDisk for BondKey {
    type Bytes = [u8; BOND_KEY_DISK_BYTES];

    fn as_bytes(&self) -> Self::Bytes {
        *self
    }
}

impl FromDisk for BondKey {
    fn from_bytes(bytes: impl AsRef<[u8]>) -> Self {
        bytes
            .as_ref()
            .try_into()
            .expect("BondKey should be exactly 32 bytes")
    }
}

impl IntoDisk for DelegationBond {
    type Bytes = Vec<u8>;

    fn as_bytes(&self) -> Self::Bytes {
        let mut bytes = Vec::with_capacity(DELEGATION_BOND_DISK_BYTES);
        bytes.extend_from_slice(&self.amount.zatoshis().to_be_bytes());
        bytes.extend_from_slice(&self.target_finalizer);
        bytes.extend_from_slice(self.created_at.as_bytes().as_ref());
        bytes
    }
}

impl FromDisk for DelegationBond {
    fn from_bytes(bytes: impl AsRef<[u8]>) -> Self {
        let bytes = bytes.as_ref();
        assert!(
            bytes.len() >= DELEGATION_BOND_DISK_BYTES,
            "DelegationBond requires at least {DELEGATION_BOND_DISK_BYTES} bytes, got {}",
            bytes.len()
        );
        let mut offset = 0;
        let amount_bytes: [u8; 8] = bytes[offset..offset + AMOUNT_DISK_BYTES]
            .try_into()
            .expect("amount should be 8 bytes");
        let amount_zatoshis = u64::from_be_bytes(amount_bytes);
        let amount = Amount::try_from(amount_zatoshis).expect("amount should be valid");
        offset += AMOUNT_DISK_BYTES;
        let target_finalizer: [u8; 32] = bytes[offset..offset + TARGET_FINALIZER_DISK_BYTES]
            .try_into()
            .expect("target_finalizer should be 32 bytes");
        offset += TARGET_FINALIZER_DISK_BYTES;
        let created_at =
            TransactionLocation::from_bytes(&bytes[offset..offset + TRANSACTION_LOCATION_DISK_BYTES]);
        Self {
            amount,
            target_finalizer,
            created_at,
        }
    }
}

impl IntoDisk for BondStatus {
    type Bytes = Vec<u8>;

    fn as_bytes(&self) -> Self::Bytes {
        let mut bytes = Vec::new();
        match self {
            BondStatus::Active => bytes.push(0),
            BondStatus::Unbonding { unbonded_at } => {
                bytes.push(1);
                bytes.extend_from_slice(unbonded_at.as_bytes().as_ref());
            }
            BondStatus::Withdrawn { withdrawn_at } => {
                bytes.push(2);
                bytes.extend_from_slice(withdrawn_at.as_bytes().as_ref());
            }
            BondStatus::Burned => bytes.push(3),
        }
        bytes
    }
}

impl FromDisk for BondStatus {
    fn from_bytes(bytes: impl AsRef<[u8]>) -> Self {
        let bytes = bytes.as_ref();
        assert!(!bytes.is_empty(), "BondStatus requires a tag byte");
        match bytes[0] {
            0 => BondStatus::Active,
            1 => {
                let unbonded_at =
                    TransactionLocation::from_bytes(&bytes[1..1 + TRANSACTION_LOCATION_DISK_BYTES]);
                BondStatus::Unbonding { unbonded_at }
            }
            2 => {
                let withdrawn_at =
                    TransactionLocation::from_bytes(&bytes[1..1 + TRANSACTION_LOCATION_DISK_BYTES]);
                BondStatus::Withdrawn { withdrawn_at }
            }
            3 => BondStatus::Burned,
            tag => panic!("Invalid BondStatus tag: {tag}"),
        }
    }
}
