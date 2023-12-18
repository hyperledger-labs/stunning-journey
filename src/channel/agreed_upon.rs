use super::{
    fixed_size_payment::{self},
    signed::SignedChannel,
    withdrawal_auth::make_signed_withdrawal_auths,
    InvalidChannel, PartIdx, Peers,
};
use crate::{
    abiencode::{
        self,
        types::{Address, Signature},
    },
    messages::{
        FunderRequestMessage, LedgerChannelFundingRequest, LedgerChannelUpdateAccepted,
        ParticipantMessage, StartWatchingLedgerChannelReq, WatcherRequestMessage,
    },
    sig,
    wire::{BroadcastMessageBus, MessageBus},
    PerunClient,
};

const ASSETS: usize = 1;
const PARTICIPANTS: usize = 2;
type State = fixed_size_payment::State<ASSETS, PARTICIPANTS>;
type Params = fixed_size_payment::Params<PARTICIPANTS>;
type Balances = fixed_size_payment::Balances<ASSETS, PARTICIPANTS>;

#[derive(Debug)]
pub enum SignError {
    AbiEncodeError(abiencode::Error),
    AlreadySigned,
}
impl From<abiencode::Error> for SignError {
    fn from(e: abiencode::Error) -> Self {
        Self::AbiEncodeError(e)
    }
}

#[derive(Debug)]
pub enum AddSignatureError {
    AbiEncodeError(abiencode::Error),
    RecoveryFailed(sig::Error),
    AlreadySigned,
    InvalidSignature(Address),
    // Used to indicate that the incomming message does not match the update.
    InvalidChannelID,
    InvalidVersionNumber,
    // Used to indicate the wrong channel was passed (i.e. a bug in the
    // application code).
    WrongVersion,
    WrongChannelId,
}
impl From<abiencode::Error> for AddSignatureError {
    fn from(e: abiencode::Error) -> Self {
        Self::AbiEncodeError(e)
    }
}
impl From<sig::Error> for AddSignatureError {
    fn from(e: sig::Error) -> Self {
        Self::RecoveryFailed(e)
    }
}
impl From<InvalidChannel> for AddSignatureError {
    fn from(e: InvalidChannel) -> Self {
        match e {
            InvalidChannel::WrongVersion => Self::WrongVersion,
            InvalidChannel::WrongChannelId => Self::WrongChannelId,
        }
    }
}

#[derive(Debug)]
pub enum BuildError {
    MissingSignatureResponse(PartIdx),
    AbiEncodeError(abiencode::Error),
}
impl From<abiencode::Error> for BuildError {
    fn from(e: abiencode::Error) -> Self {
        Self::AbiEncodeError(e)
    }
}

#[derive(Debug)]
pub struct AgreedUponChannel<'cl, B: MessageBus> {
    part_idx: PartIdx,
    withdraw_receiver: Address,
    client: &'cl PerunClient<B>,
    funding_agreement: Balances,
    init_state: State,
    params: Params,
    signatures: [Option<Signature>; 2],
    peers: Peers,
}

impl<'cl, B: MessageBus> AgreedUponChannel<'cl, B> {
    pub(super) fn new(
        client: &'cl PerunClient<B>,
        funding_agreement: Balances,
        part_idx: PartIdx,
        withdraw_receiver: Address,
        init_state: State,
        params: Params,
        peers: Peers,
    ) -> Self {
        AgreedUponChannel {
            part_idx,
            client,
            withdraw_receiver,
            funding_agreement,
            init_state,
            params,
            signatures: [None; PARTICIPANTS],
            peers,
        }
    }

    pub fn sign(&mut self) -> Result<(), SignError> {
        match self.signatures[self.part_idx] {
            Some(_) => Err(SignError::AlreadySigned),
            None => {
                // Sign the initial state
                let hash = abiencode::to_hash(&self.init_state)?;
                let sig = self.client.signer.sign_eth(hash);
                // Add signature to the proposed channel
                self.signatures[self.part_idx] = Some(sig);
                // Send to other participants
                self.client.bus.broadcast_to_participants(
                    self.part_idx,
                    &self.peers,
                    ParticipantMessage::ChannelUpdateAccepted(LedgerChannelUpdateAccepted {
                        channel: self.init_state.channel_id(),
                        version: self.init_state.version(),
                        sig,
                    }),
                );
                Ok(())
            }
        }
    }

    // This function allows adding our own signature if we really want. There is
    // currently no easy way to get one, but it is possible.
    pub fn add_signature(
        &mut self,
        msg: LedgerChannelUpdateAccepted,
    ) -> Result<(), AddSignatureError> {
        if msg.channel != self.init_state.channel_id() {
            return Err(AddSignatureError::InvalidChannelID);
        }
        if msg.version != 0 {
            return Err(AddSignatureError::InvalidVersionNumber);
        }

        let hash = abiencode::to_hash(&self.init_state)?;
        let signer = self.client.signer.recover_signer(hash, msg.sig)?;

        // Verify signature is comming from a valid participant.
        //
        // There is currently a difference to go-perun, which gets the
        // participant index by comparing `wire.Address`-es instead of ephemeral
        // `wallet.Address`-es, then compares only against one `wallet.Address`.
        // As long as both are unique this doesn't make a difference (not even
        // in performance). This means that channels where multiple participants
        // have the same channel key would be problematic in Rust, while it
        // would be perfectly fine in Go (except that it wouldn't be a good idea
        // to do that). On the other side, Rust would allow multiple
        // participants with the same wire identity (which doesn't really make
        // sense either).
        let (part_idx, _) = self
            .params
            .participants
            .iter()
            .enumerate()
            .find(|(_, &addr)| addr == signer)
            .ok_or(AddSignatureError::InvalidSignature(signer))?;

        match self.signatures[part_idx] {
            Some(_) => Err(AddSignatureError::AlreadySigned),
            None => {
                self.signatures[part_idx] = Some(msg.sig);
                Ok(())
            }
        }
    }

    pub fn build(self) -> Result<SignedChannel<'cl, B>, (Self, BuildError)> {
        // Make sure we have the signature from all participants. They have
        // already been verified in `add_signature()` or we created it ourselves
        // with `sign()`. At the same time, this loop collects the signatures
        // for the next phase into an array.
        let mut signatures: [Signature; PARTICIPANTS] = [Signature::default(); PARTICIPANTS];
        for (part_idx, s) in self.signatures.iter().enumerate() {
            signatures[part_idx] = match s {
                Some(v) => *v,
                None => return Err((self, BuildError::MissingSignatureResponse(part_idx))),
            };
        }

        self.client
            .bus
            .send_to_watcher(WatcherRequestMessage::WatchRequest(
                StartWatchingLedgerChannelReq {
                    params: self.params,
                    state: self.init_state,
                    sigs: signatures,
                },
            ));

        self.client
            .bus
            .send_to_funder(FunderRequestMessage::FundReq(LedgerChannelFundingRequest {
                part_idx: self.part_idx,
                funding_agreement: self.funding_agreement,
                params: self.params,
                state: self.init_state,
            }));

        Ok(SignedChannel::new(
            self.client,
            self.part_idx,
            self.withdraw_receiver,
            self.init_state,
            self.params,
            signatures,
            self.peers,
        ))
    }
}

impl<'cl, B: MessageBus> TryFrom<AgreedUponChannel<'cl, B>> for SignedChannel<'cl, B> {
    type Error = (AgreedUponChannel<'cl, B>, BuildError);

    fn try_from(value: AgreedUponChannel<'cl, B>) -> Result<Self, Self::Error> {
        value.build()
    }
}
