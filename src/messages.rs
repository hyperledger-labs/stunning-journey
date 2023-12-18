mod funding_request;
mod proposal;
mod update;
mod watch_request;

pub use funding_request::{
    AdjudicatorReq, LedgerChannelFundingRequest, RegisterReq, Transaction, WithdrawReq,
};
pub use proposal::{LedgerChannelProposal, LedgerChannelProposalAcc};
pub use update::{LedgerChannelUpdate, LedgerChannelUpdateAccepted};
pub use watch_request::{SignedWithdrawalAuth, StartWatchingLedgerChannelReq, WatchInfo};

use crate::abiencode::types::Hash;
use alloc::string::String;

#[derive(Debug)]
pub enum ConversionError {
    ParticipantSizeMissmatch,
    AssetSizeMissmatch,
    ByteLengthMissmatch,
    ExptectedSome,
    StateChannelsNotSupported,
}

/// Messages sent to the Watcher service.
#[derive(Debug)]
pub enum WatcherRequestMessage {
    /// Ask the Watcher to start watching the blockchain for disputes.
    /// Acknowledged with [WatcherReplyMessage::Ack] containing `version == 0`.
    WatchRequest(StartWatchingLedgerChannelReq),
}

/// Messages sent from the Watcher service.
#[derive(Debug)]
pub enum WatcherReplyMessage {
    /// Reply from the Watcher that a state has been received and will be used
    /// in a dispute case.
    Ack { id: Hash, version: u64 },
    /// Ask the Watcher to initialize a dispute on-chain, with the given state.
    /// It currently does not contain the parameters for reducing the amount of
    /// communication needed. Adding it might be useful to make the watcher less
    /// stateful.
    DisputeAck { id: Hash },
    /// Used by the Watcher to notify the device of the existence of an on-chain
    /// dispute. This way the device knows that it does not/should not continue
    /// updating the channel.
    DisputeNotification { id: Hash },
}

/// Messages sent to the Funder service.
#[derive(Debug)]
pub enum FunderRequestMessage {
    FundReq(LedgerChannelFundingRequest),
    RegisterReq(RegisterReq),
    WithdrawReq(WithdrawReq),
}

/// Messages sent from the Funder service.
#[derive(Debug)]
pub enum FunderReplyMessage {
    Funded { id: Hash },
}

/// Messages sent between participants of a channel.
#[derive(Debug, Clone)]
pub enum ParticipantMessage {
    Auth,
    ChannelProposal(LedgerChannelProposal),
    ProposalAccepted(LedgerChannelProposalAcc),
    ProposalRejected {
        id: Hash,
        reason: String,
    },
    ChannelUpdate(LedgerChannelUpdate),
    ChannelUpdateAccepted(LedgerChannelUpdateAccepted),
    ChannelUpdateRejected {
        id: Hash,
        version: u64,
        reason: String,
    },
}
