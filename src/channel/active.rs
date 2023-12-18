use super::{
    channel_update::ChannelUpdate,
    fixed_size_payment::{self},
    withdrawal_auth, PartIdx, Peers, SignError,
};
use crate::{
    abiencode::{
        self,
        types::{Address, Hash, Signature},
    },
    messages::{
        LedgerChannelUpdate,
        ParticipantMessage,
        StartWatchingLedgerChannelReq,
        WatcherRequestMessage,
    },
    sig,
    wire::{BroadcastMessageBus, MessageBus},
    PerunClient,
};

const ASSETS: usize = 1;
const PARTICIPANTS: usize = 2;
type State = fixed_size_payment::State<ASSETS, PARTICIPANTS>;
type Params = fixed_size_payment::Params<PARTICIPANTS>;

#[derive(Debug)]
pub enum ProposeUpdateError {
    AbiEncodeError(abiencode::Error),
    InvalidUpdate(InvalidUpdate),
}
impl From<abiencode::Error> for ProposeUpdateError {
    fn from(e: abiencode::Error) -> Self {
        Self::AbiEncodeError(e)
    }
}
impl From<InvalidUpdate> for ProposeUpdateError {
    fn from(e: InvalidUpdate) -> Self {
        Self::InvalidUpdate(e)
    }
}

#[derive(Debug)]
pub enum HandleUpdateError {
    AbiEncodeError(abiencode::Error),
    RecoveryFailed(sig::Error),
    InvalidSignature(Address),
    InvalidUpdate(InvalidUpdate),
}
impl From<abiencode::Error> for HandleUpdateError {
    fn from(e: abiencode::Error) -> Self {
        Self::AbiEncodeError(e)
    }
}
impl From<sig::Error> for HandleUpdateError {
    fn from(e: sig::Error) -> Self {
        Self::RecoveryFailed(e)
    }
}
impl From<InvalidUpdate> for HandleUpdateError {
    fn from(e: InvalidUpdate) -> Self {
        Self::InvalidUpdate(e)
    }
}

#[derive(Debug)]
pub enum InvalidUpdate {
    InvalidChannelID,
    InvalidVersionNumber,
    CurrentStateIsFinal,
    AssetsMismatch,
    TotalAllocationAmountMismatch,
}

#[derive(Debug)]
pub struct ActiveChannel<'cl, B: MessageBus> {
    part_idx: PartIdx,
    withdraw_receiver: Address,
    client: &'cl PerunClient<B>,
    state: State,
    params: Params,
    signatures: [Signature; PARTICIPANTS],
    peers: Peers,
}

impl<'cl, B: MessageBus> ActiveChannel<'cl, B> {
    pub(super) fn new(
        client: &'cl PerunClient<B>,
        part_idx: PartIdx,
        withdraw_receiver: Address,
        init_state: State,
        params: Params,
        signatures: [Signature; PARTICIPANTS],
        peers: Peers,
    ) -> Self {
        debug_assert!(part_idx < params.participants.len());

        ActiveChannel {
            part_idx,
            client,
            state: init_state,
            params,
            signatures,
            withdraw_receiver,
            peers,
        }
    }

    pub fn channel_id(&self) -> Hash {
        self.state.channel_id()
    }
    pub fn version(&self) -> u64 {
        self.state.version()
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn part_idx(&self) -> PartIdx {
        self.part_idx
    }

    pub fn client(&self) -> &PerunClient<B> {
        self.client
    }

    pub fn peers(&self) -> &Peers {
        &self.peers
    }

    pub fn params(&self) -> Params {
        self.params
    }

    fn check_valid_transition(&self, new_state: State) -> Result<(), InvalidUpdate> {
        debug_assert_eq!(new_state.outcome.locked.len(), 0, "At the moment we don't support subchannels and thus don't represent locked balances. This assert exists for when we do add it, thus warning us if this 'we don't have locked values' assumption changes. If it does: Go-Perun asserts that the `SubAlloc` (locked values) are equivalent and did not change, see `validTwoPartyUpdate`.");
        new_state.outcome.debug_assert_valid();

        if new_state.channel_id() != self.state.channel_id() {
            Err(InvalidUpdate::InvalidChannelID)
        } else if self.state.is_final {
            Err(InvalidUpdate::CurrentStateIsFinal)
        } else if new_state.version() != self.state.version() + 1 {
            Err(InvalidUpdate::InvalidVersionNumber)
        } else if new_state.outcome.assets != self.state.outcome.assets {
            Err(InvalidUpdate::AssetsMismatch)
        } else if new_state.outcome.total_assets() != self.state.outcome.total_assets() {
            Err(InvalidUpdate::TotalAllocationAmountMismatch)
        } else {
            Ok(())
        }
    }

    pub fn update(&self, new_state: State) -> Result<ChannelUpdate, ProposeUpdateError> {
        self.check_valid_transition(new_state)?;

        // Sign immediately, we need the signature to send the proposal.
        let hash = abiencode::to_hash(&new_state)?;
        let sig = self.client.signer.sign_eth(hash);
        self.client.bus.broadcast_to_participants(
            self.part_idx,
            &self.peers,
            ParticipantMessage::ChannelUpdate(LedgerChannelUpdate {
                state: new_state,
                actor_idx: self.part_idx,
                sig,
            }),
        );

        Ok(ChannelUpdate::new(self, new_state, self.part_idx, sig))
    }

    pub fn handle_update(
        &self,
        msg: LedgerChannelUpdate,
    ) -> Result<ChannelUpdate, HandleUpdateError> {
        self.check_valid_transition(msg.state)?;

        let hash = abiencode::to_hash(&msg.state)?;
        let signer = self.client.signer.recover_signer(hash, msg.sig)?;

        if self.params.participants[msg.actor_idx] != signer {
            return Err(HandleUpdateError::InvalidSignature(signer));
        }

        Ok(ChannelUpdate::new(self, msg.state, msg.actor_idx, msg.sig))
    }

    pub(super) fn force_update(
        &mut self,
        new_state: State,
        signatures: [Signature; PARTICIPANTS],
    ) -> Result<(), SignError> {
        // To prevent modifying self (the channel state+signatures) in case
        // send_current_state_to_watcher returns an Error we roll-back the
        // changes made here. At the moment this could only happen if we can't
        // abiencode the WithdrawalAuth message, but in the future this might
        // include Errors from the MessageBus.
        //
        // Alternatives considered:
        // - Make `make_watch_update` independent of self (associated funciton
        //   instead of member function). This would mean copying all the
        //   parameters to its call location (the function existed to not do
        //   that).
        // - Copy content of `make_watch_update` here (tried to avoid code
        //   duplication)
        // - give only the new state+signatures ot `make_watch_update` instead
        //   of rolling back. This would make the correctness dependent on
        //   whether `make_watch_update` uses the state passed via arguments or
        //   that in self, making it easy to use the wrong one and easy to not
        //   see that as a bug (especially since all other values are read from
        //   self) and this one is the exception.
        let old_state = self.state;
        let old_sigs = self.signatures;
        self.state = new_state;
        self.signatures = signatures;

        match self.send_current_state_to_watcher() {
            Ok(_) => Ok(()),
            Err(e) => {
                self.state = old_state;
                self.signatures = old_sigs;
                Err(e)
            }
        }
    }

    fn make_watch_info(&self) -> Result<StartWatchingLedgerChannelReq, SignError> {
        Ok(StartWatchingLedgerChannelReq {
            params: self.params,
            state: self.state,
            sigs: self.signatures,
        })
    }

    pub fn send_current_state_to_watcher(&self) -> Result<(), SignError> {
        self.client
            .bus
            .send_to_watcher(WatcherRequestMessage::WatchRequest(self.make_watch_info()?));
        Ok(())
    }

    // Use `update()` if the state has to change, too

    pub fn close_normal(&self) -> Result<ChannelUpdate, ProposeUpdateError> {
        let mut new_state = self.state.make_next_state();
        new_state.is_final = true;
        self.update(new_state)
    }

    // At the moment this just drops the channel after sending the message. In
    // the future it might make sense to have a struct representing a closing
    // channel, for example to allow resending the last message.
    pub fn force_close(self) -> Result<(), (Self, SignError)> {
        let watch_info = match self.make_watch_info() {
            Ok(v) => v,
            Err(e) => return Err((self, e)),
        };
        self.client
            .bus
            .send_to_watcher(WatcherRequestMessage::StartDispute(watch_info));
        Ok(())
    }

    // At the moment this just drops the channel. In the future it might make
    // sense to have a struct representing a closing channel, for example to
    // allow resending the last message.
    pub fn handle_dispute(self) {}
}
