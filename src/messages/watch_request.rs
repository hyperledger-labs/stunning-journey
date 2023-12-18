use super::ConversionError;
use crate::{
    abiencode::types::Signature,
    channel::{fixed_size_payment, PartIdx},
    perunwire, Address,
};

const ASSETS: usize = 1;
const PARTICIPANTS: usize = 2;
type State = fixed_size_payment::State<ASSETS, PARTICIPANTS>;
type Params = fixed_size_payment::Params<PARTICIPANTS>;

#[derive(Debug, Clone, Copy)]
pub struct WatchInfo {
    pub part_idx: PartIdx,
    pub params: Params,
    pub state: State,
    pub signatures: [Signature; PARTICIPANTS],
    pub withdrawal_auths: [SignedWithdrawalAuth; ASSETS],
}

#[derive(Debug, Clone, Copy)]
pub struct StartWatchingLedgerChannelReq {
    pub params: Params,
    pub state: State,
    pub sigs: [Signature; PARTICIPANTS],
}

impl TryFrom<perunwire::StartWatchingLedgerChannelReq> for StartWatchingLedgerChannelReq {
    type Error = ConversionError;

    fn try_from(value: perunwire::StartWatchingLedgerChannelReq) -> Result<Self, Self::Error> {
        if value.sigs.len() != PARTICIPANTS {
            return Err(ConversionError::ParticipantSizeMissmatch);
        }

        let mut sigs = [Signature::default(); PARTICIPANTS];
        for (a, b) in sigs.iter_mut().zip(value.sigs) {
            *a = Signature(b.try_into().or(Err(ConversionError::ByteLengthMissmatch))?);
        }

        Ok(Self {
            params: value
                .params
                .ok_or(ConversionError::ExptectedSome)?
                .try_into()?,
            state: value
                .state
                .ok_or(ConversionError::ExptectedSome)?
                .try_into()?,
            sigs,
        })
    }
}

impl From<StartWatchingLedgerChannelReq> for perunwire::StartWatchingLedgerChannelReq {
    fn from(value: StartWatchingLedgerChannelReq) -> Self {
        Self {
            session_id: "".into(),
            params: Some(value.params.into()),
            state: Some(value.state.into()),
            sigs: value.sigs.map(|sig| sig.0.to_vec()).to_vec(),
        }
    }
}

impl TryFrom<perunwire::WatchRequestMsg> for WatchInfo {
    type Error = ConversionError;

    fn try_from(value: perunwire::WatchRequestMsg) -> Result<Self, Self::Error> {
        let signed_state = value.state.ok_or(ConversionError::ExptectedSome)?;

        if signed_state.sigs.len() != PARTICIPANTS {
            return Err(ConversionError::ParticipantSizeMissmatch);
        }
        if value.withdrawal_auths.len() != ASSETS {
            return Err(ConversionError::ParticipantSizeMissmatch);
        }

        let mut signatures = [Signature::default(); PARTICIPANTS];
        for (a, b) in signatures.iter_mut().zip(signed_state.sigs) {
            *a = Signature(b.try_into().or(Err(ConversionError::ByteLengthMissmatch))?);
        }

        let mut withdrawal_auths = [SignedWithdrawalAuth::default(); ASSETS];
        for (a, b) in withdrawal_auths.iter_mut().zip(value.withdrawal_auths) {
            *a = b.try_into()?;
        }

        Ok(Self {
            part_idx: value.participant as usize,
            params: signed_state
                .params
                .ok_or(ConversionError::ExptectedSome)?
                .try_into()?,
            state: signed_state
                .state
                .ok_or(ConversionError::ExptectedSome)?
                .try_into()?,
            signatures,
            withdrawal_auths,
        })
    }
}

impl From<WatchInfo> for perunwire::WatchRequestMsg {
    fn from(value: WatchInfo) -> Self {
        Self {
            participant: value.part_idx as u32,
            state: Some(perunwire::SignedState {
                params: Some(value.params.into()),
                state: Some(value.state.into()),
                sigs: value.signatures.map(|sig| sig.0.to_vec()).to_vec(),
            }),
            withdrawal_auths: value.withdrawal_auths.map(|a| a.into()).to_vec(),
        }
    }
}

impl TryFrom<perunwire::ForceCloseRequestMsg> for WatchInfo {
    type Error = ConversionError;

    fn try_from(value: perunwire::ForceCloseRequestMsg) -> Result<Self, Self::Error> {
        value
            .latest
            .ok_or(ConversionError::ExptectedSome)?
            .try_into()
    }
}

impl From<WatchInfo> for perunwire::ForceCloseRequestMsg {
    fn from(value: WatchInfo) -> Self {
        Self {
            channel_id: value.state.channel_id().0.to_vec(),
            latest: Some(value.into()),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SignedWithdrawalAuth {
    pub sig: Signature,
    pub receiver: Address,
}

impl TryFrom<perunwire::SignedWithdrawalAuth> for SignedWithdrawalAuth {
    type Error = ConversionError;

    fn try_from(value: perunwire::SignedWithdrawalAuth) -> Result<Self, Self::Error> {
        Ok(Self {
            sig: Signature(
                value
                    .sig
                    .try_into()
                    .or(Err(ConversionError::ByteLengthMissmatch))?,
            ),
            receiver: Address(
                value
                    .receiver
                    .try_into()
                    .or(Err(ConversionError::ByteLengthMissmatch))?,
            ),
        })
    }
}

impl From<SignedWithdrawalAuth> for perunwire::SignedWithdrawalAuth {
    fn from(value: SignedWithdrawalAuth) -> Self {
        Self {
            sig: value.sig.0.to_vec(),
            receiver: value.receiver.0.to_vec(),
        }
    }
}
