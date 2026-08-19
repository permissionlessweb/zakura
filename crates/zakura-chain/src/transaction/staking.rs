//! Crosslink staking action (ShieldedLabs zebra-crosslink / librustzcash fork).
//!
//! Wire format is a tagged optional: `0` = none, else kind (1–5) + le u64 +
//! 32-byte target + 32-byte source + u8-len names.

use std::io::{self, Read, Write};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

/// Roster mutation kind. `0` is reserved for `None` on the wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
#[cfg_attr(any(test, feature = "proptest-impl"), derive(Serialize))]
pub enum StakingActionKind {
    /// Add `val` zats of voting power to `target`.
    Add = 1,
    /// Subtract `val` from `target`.
    Sub = 2,
    /// Set `target` power to `val` (must be ≤ current).
    Clear = 3,
    /// Move `val` from `source` to `target`.
    Move = 4,
    /// Clear `source` down to `val` and add the delta to `target`.
    MoveClear = 5,
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
            1 => Ok(Self::Add),
            2 => Ok(Self::Sub),
            3 => Ok(Self::Clear),
            4 => Ok(Self::Move),
            5 => Ok(Self::MoveClear),
            _ => Err(()),
        }
    }
}

/// One roster command carried on a `VCrosslink` transaction.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
#[cfg_attr(any(test, feature = "proptest-impl"), derive(Serialize))]
pub struct StakingAction {
    /// Kind of roster mutation.
    pub kind: StakingActionKind,
    /// Voting-power amount in zats.
    pub val: u64,
    /// Destination finalizer public key.
    pub target: [u8; 32],
    /// Source finalizer public key (move / move-clear).
    pub source: [u8; 32],
    /// Lab identity string used to derive `target`.
    pub insecure_target_name: String,
    /// Lab identity string used to derive `source`.
    pub insecure_source_name: String,
}

impl StakingAction {
    /// `ADD|val|name`, `SUB|…`, `CLR|…`, `MOV|val|target|source`, `MCL|…`.
    pub fn to_cmd_string(&self) -> String {
        let kind = match self.kind {
            StakingActionKind::Add => "ADD",
            StakingActionKind::Sub => "SUB",
            StakingActionKind::Clear => "CLR",
            StakingActionKind::Move => "MOV",
            StakingActionKind::MoveClear => "MCL",
        };
        let mut s = format!("{kind}|{}|{}", self.val, self.insecure_target_name);
        if matches!(
            self.kind,
            StakingActionKind::Move | StakingActionKind::MoveClear
        ) {
            s.push('|');
            s.push_str(&self.insecure_source_name);
        }
        s
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
        let val = reader.read_u64::<LittleEndian>()?;
        let mut target = [0u8; 32];
        reader.read_exact(&mut target)?;
        let mut source = [0u8; 32];
        reader.read_exact(&mut source)?;
        let target_name_len = reader.read_u8()? as usize;
        let mut target_name_buf = vec![0u8; target_name_len];
        reader.read_exact(&mut target_name_buf)?;
        let source_name_len = reader.read_u8()? as usize;
        let mut source_name_buf = vec![0u8; source_name_len];
        reader.read_exact(&mut source_name_buf)?;
        Ok(Some(Self {
            kind,
            val,
            target,
            source,
            insecure_target_name: String::from_utf8_lossy(&target_name_buf).into_owned(),
            insecure_source_name: String::from_utf8_lossy(&source_name_buf).into_owned(),
        }))
    }

    /// Write an optional staking action.
    pub fn write<W: Write>(staking_action: &Option<Self>, mut writer: W) -> io::Result<()> {
        match staking_action {
            None => writer.write_u8(0),
            Some(action) => {
                writer.write_u8(u8::from(action.kind))?;
                writer.write_u64::<LittleEndian>(action.val)?;
                writer.write_all(&action.target)?;
                writer.write_all(&action.source)?;
                let target_len = action.insecure_target_name.len().min(u8::MAX as usize);
                writer.write_u8(target_len as u8)?;
                writer.write_all(&action.insecure_target_name.as_bytes()[..target_len])?;
                let source_len = action.insecure_source_name.len().min(u8::MAX as usize);
                writer.write_u8(source_len as u8)?;
                writer.write_all(&action.insecure_source_name.as_bytes()[..source_len])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staking_action_round_trips_none_and_add() {
        let mut buf = Vec::new();
        StakingAction::write(&None, &mut buf).unwrap();
        assert_eq!(StakingAction::read(&buf[..]).unwrap(), None);

        let action = StakingAction {
            kind: StakingActionKind::Add,
            val: 42,
            target: [7u8; 32],
            source: [0u8; 32],
            insecure_target_name: "alice".into(),
            insecure_source_name: String::new(),
        };
        buf.clear();
        StakingAction::write(&Some(action.clone()), &mut buf).unwrap();
        let back = StakingAction::read(&buf[..]).unwrap().unwrap();
        assert_eq!(back, action);
        assert_eq!(back.to_cmd_string(), "ADD|42|alice");
    }
}
