mod active;
mod agreed_upon;
mod channel_update;
pub mod fixed_size_payment;
mod proposal;
mod signed;
mod withdrawal_auth;

use crate::abiencode::types::{Address, Bytes32, U256};
use alloc::vec::Vec;
use serde::Serialize;

pub use active::*;
pub use agreed_upon::*;
pub use channel_update::*;
pub use channel_update::*;
pub use proposal::*;
pub use signed::*;

// Re-exported because it is part of the low-level channel API
pub use crate::messages::LedgerChannelProposal;

/// Index of a participant in the channel.
///
/// `0` is the proposer of the channel.
pub type PartIdx = usize;

/// The nonce added by each participant.
///
/// They are combined into a single [U256] using SHA256.
pub type NonceShare = Bytes32;

/// Uniquely identifies an Asset by blockchain + AssetHolder.
#[derive(Serialize, Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct Asset {
    pub holder: Address,
}

pub type Peers = Vec<Vec<u8>>;
