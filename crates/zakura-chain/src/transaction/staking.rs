//! Crosslink staking action (ShieldedLabs crosslink_monolith v13).
//!
//! Wire format is a tagged optional: `0` = none, else a kind-specific
//! pubkey / challenge / signature blob. This is the Season 1 hasher, not
//! the older ADD/SUB/CLR layout.

use std::io::{self, Read, Write};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

/// Season 1 staking kind. `0` is reserved for `None` on the wire.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
#[cfg_attr(any(test, feature = "proptest-impl"), derive(Serialize))]
pub enum StakingActionKind {
    #[default]
    Null = 0,
    CreateNewDelegationBond = 1,
    BeginDelegationUnbonding = 2,
    WithdrawDelegationBond = 3,
    RetargetDelegationBond = 4,
    RegisterFinalizer = 5,
    ConvertFinalizerRewardToDelegationBond = 6,
    UpdateFinalizerKey = 7,
}

impl From<StakingActionKind> for u8 {
    fn from(kind: StakingActionKind) -> u8 {
        kind as u8
    }
}

impl TryFrom<u8> for StakingActionKind {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, ()> {
        match value {
            0 => Ok(Self::Null),
            1 => Ok(Self::CreateNewDelegationBond),
            2 => Ok(Self::BeginDelegationUnbonding),
            3 => Ok(Self::WithdrawDelegationBond),
            4 => Ok(Self::RetargetDelegationBond),
            5 => Ok(Self::RegisterFinalizer),
            6 => Ok(Self::ConvertFinalizerRewardToDelegationBond),
            7 => Ok(Self::UpdateFinalizerKey),
            _ => Err(()),
        }
    }
}

/// One staking command on a `VCrosslink` transaction.
///
/// Field names match v13 `zcash_primitives::transaction::StakingAction`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct StakingAction {
    pub kind: StakingActionKind,
    pub amount_zats: u64,
    pub arg32_0: [u8; 32],
    pub arg32_1: [u8; 32],
    pub arg32_2: [u8; 32],
    pub arg32_3: [u8; 32],
    pub arg64_0: [u8; 64],
    pub arg64_1: [u8; 64],
}

impl Default for StakingAction {
    fn default() -> Self {
        Self {
            kind: StakingActionKind::Null,
            amount_zats: 0,
            arg32_0: [0; 32],
            arg32_1: [0; 32],
            arg32_2: [0; 32],
            arg32_3: [0; 32],
            arg64_0: [0; 64],
            arg64_1: [0; 64],
        }
    }
}

impl StakingAction {
    /// Bytes committed under `ZTxCrosslinkHash` (v13 `hash_to_state`).
    pub fn txid_preimage(&self) -> Option<Vec<u8>> {
        let mut buf = Vec::with_capacity(1 + 32 + 32 + 64 + 32 + 8);
        buf.push(u8::from(self.kind));
        match self.kind {
            StakingActionKind::CreateNewDelegationBond => {
                buf.extend_from_slice(&self.arg32_0);
                buf.extend_from_slice(&self.arg32_1);
                buf.extend_from_slice(&self.arg64_0);
                buf.extend_from_slice(&self.arg32_2);
                buf.extend_from_slice(&self.amount_zats.to_le_bytes());
            }
            StakingActionKind::BeginDelegationUnbonding | StakingActionKind::RegisterFinalizer => {
                buf.extend_from_slice(&self.arg32_0);
                buf.extend_from_slice(&self.arg32_1);
                buf.extend_from_slice(&self.arg64_0);
            }
            StakingActionKind::WithdrawDelegationBond => {
                buf.extend_from_slice(&self.arg32_0);
                buf.extend_from_slice(&self.arg32_1);
                buf.extend_from_slice(&self.arg64_0);
                buf.extend_from_slice(&self.amount_zats.to_le_bytes());
            }
            StakingActionKind::RetargetDelegationBond => {
                buf.extend_from_slice(&self.arg32_0);
                buf.extend_from_slice(&self.arg32_1);
                buf.extend_from_slice(&self.arg64_0);
                buf.extend_from_slice(&self.arg32_2);
            }
            StakingActionKind::ConvertFinalizerRewardToDelegationBond => {
                buf.extend_from_slice(&self.arg32_0);
                buf.extend_from_slice(&self.arg32_1);
                buf.extend_from_slice(&self.arg64_0);
                buf.extend_from_slice(&self.arg32_2);
                buf.extend_from_slice(&self.amount_zats.to_le_bytes());
                buf.extend_from_slice(&self.arg32_3);
                buf.extend_from_slice(&self.arg64_1);
            }
            StakingActionKind::UpdateFinalizerKey => {
                buf.extend_from_slice(&self.arg32_0);
                buf.extend_from_slice(&self.arg32_1);
                buf.extend_from_slice(&self.arg64_0);
                buf.extend_from_slice(&self.arg32_2);
                buf.extend_from_slice(&self.arg32_3);
                buf.extend_from_slice(&self.arg64_1);
            }
            StakingActionKind::Null => return None,
        }
        Some(buf)
    }

    /// Read an optional staking action (tag `0` = none).
    pub fn read<R: Read>(mut reader: R) -> io::Result<Option<Self>> {
        let tag = reader.read_u8()?;
        if tag == 0 {
            return Ok(None);
        }
        let kind = StakingActionKind::try_from(tag).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unexpected staking action tag: {tag}"),
            )
        })?;
        let mut ret = Self {
            kind,
            ..Self::default()
        };
        match kind {
            StakingActionKind::Null => return Ok(None),
            StakingActionKind::CreateNewDelegationBond => {
                reader.read_exact(&mut ret.arg32_0)?;
                reader.read_exact(&mut ret.arg32_1)?;
                reader.read_exact(&mut ret.arg64_0)?;
                reader.read_exact(&mut ret.arg32_2)?;
                ret.amount_zats = reader.read_u64::<LittleEndian>()?;
            }
            StakingActionKind::BeginDelegationUnbonding | StakingActionKind::RegisterFinalizer => {
                reader.read_exact(&mut ret.arg32_0)?;
                reader.read_exact(&mut ret.arg32_1)?;
                reader.read_exact(&mut ret.arg64_0)?;
            }
            StakingActionKind::WithdrawDelegationBond => {
                reader.read_exact(&mut ret.arg32_0)?;
                reader.read_exact(&mut ret.arg32_1)?;
                reader.read_exact(&mut ret.arg64_0)?;
                ret.amount_zats = reader.read_u64::<LittleEndian>()?;
            }
            StakingActionKind::RetargetDelegationBond => {
                reader.read_exact(&mut ret.arg32_0)?;
                reader.read_exact(&mut ret.arg32_1)?;
                reader.read_exact(&mut ret.arg64_0)?;
                reader.read_exact(&mut ret.arg32_2)?;
            }
            StakingActionKind::ConvertFinalizerRewardToDelegationBond => {
                reader.read_exact(&mut ret.arg32_0)?;
                reader.read_exact(&mut ret.arg32_1)?;
                reader.read_exact(&mut ret.arg64_0)?;
                reader.read_exact(&mut ret.arg32_2)?;
                ret.amount_zats = reader.read_u64::<LittleEndian>()?;
                reader.read_exact(&mut ret.arg32_3)?;
                reader.read_exact(&mut ret.arg64_1)?;
            }
            StakingActionKind::UpdateFinalizerKey => {
                reader.read_exact(&mut ret.arg32_0)?;
                reader.read_exact(&mut ret.arg32_1)?;
                reader.read_exact(&mut ret.arg64_0)?;
                reader.read_exact(&mut ret.arg32_2)?;
                reader.read_exact(&mut ret.arg32_3)?;
                reader.read_exact(&mut ret.arg64_1)?;
            }
        }
        Ok(Some(ret))
    }

    /// Write an optional staking action.
    pub fn write<W: Write>(staking_action: &Option<Self>, mut writer: W) -> io::Result<()> {
        let Some(action) = staking_action else {
            return writer.write_u8(0);
        };
        match action.kind {
            StakingActionKind::Null => writer.write_u8(0),
            StakingActionKind::CreateNewDelegationBond => {
                writer.write_u8(u8::from(action.kind))?;
                writer.write_all(&action.arg32_0)?;
                writer.write_all(&action.arg32_1)?;
                writer.write_all(&action.arg64_0)?;
                writer.write_all(&action.arg32_2)?;
                writer.write_u64::<LittleEndian>(action.amount_zats)
            }
            StakingActionKind::BeginDelegationUnbonding | StakingActionKind::RegisterFinalizer => {
                writer.write_u8(u8::from(action.kind))?;
                writer.write_all(&action.arg32_0)?;
                writer.write_all(&action.arg32_1)?;
                writer.write_all(&action.arg64_0)
            }
            StakingActionKind::WithdrawDelegationBond => {
                writer.write_u8(u8::from(action.kind))?;
                writer.write_all(&action.arg32_0)?;
                writer.write_all(&action.arg32_1)?;
                writer.write_all(&action.arg64_0)?;
                writer.write_u64::<LittleEndian>(action.amount_zats)
            }
            StakingActionKind::RetargetDelegationBond => {
                writer.write_u8(u8::from(action.kind))?;
                writer.write_all(&action.arg32_0)?;
                writer.write_all(&action.arg32_1)?;
                writer.write_all(&action.arg64_0)?;
                writer.write_all(&action.arg32_2)
            }
            StakingActionKind::ConvertFinalizerRewardToDelegationBond => {
                writer.write_u8(u8::from(action.kind))?;
                writer.write_all(&action.arg32_0)?;
                writer.write_all(&action.arg32_1)?;
                writer.write_all(&action.arg64_0)?;
                writer.write_all(&action.arg32_2)?;
                writer.write_u64::<LittleEndian>(action.amount_zats)?;
                writer.write_all(&action.arg32_3)?;
                writer.write_all(&action.arg64_1)
            }
            StakingActionKind::UpdateFinalizerKey => {
                writer.write_u8(u8::from(action.kind))?;
                writer.write_all(&action.arg32_0)?;
                writer.write_all(&action.arg32_1)?;
                writer.write_all(&action.arg64_0)?;
                writer.write_all(&action.arg32_2)?;
                writer.write_all(&action.arg32_3)?;
                writer.write_all(&action.arg64_1)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staking_action_round_trips_none_and_create_bond() {
        let mut buf = Vec::new();
        StakingAction::write(&None, &mut buf).unwrap();
        assert_eq!(StakingAction::read(&buf[..]).unwrap(), None);

        let mut action = StakingAction {
            kind: StakingActionKind::CreateNewDelegationBond,
            amount_zats: 42,
            ..StakingAction::default()
        };
        action.arg32_0 = [1u8; 32];
        action.arg32_1 = [2u8; 32];
        action.arg64_0 = [3u8; 64];
        action.arg32_2 = [4u8; 32];
        buf.clear();
        StakingAction::write(&Some(action), &mut buf).unwrap();
        let back = StakingAction::read(&buf[..]).unwrap().unwrap();
        assert_eq!(back, action);
        assert_eq!(buf.len(), 1 + 32 + 32 + 64 + 32 + 8);
    }
}
