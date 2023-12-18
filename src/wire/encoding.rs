use prost::{bytes::BufMut, EncodeError};

use super::{BytesBus, Identity, MessageBus, ParticipantMessage};
use crate::{
    messages::{FunderRequestMessage, WatcherRequestMessage},
    perunwire::{
        envelope, message, AuthResponseMsg, ChannelProposalRejMsg, ChannelUpdateRejMsg, Envelope,
        Message,
    },
};
use alloc::vec::Vec;

#[derive(Debug)]
pub struct ProtoBufEncodingLayer<B: BytesBus> {
    pub bus: B,
}

impl<B: BytesBus> ProtoBufEncodingLayer<B> {
    fn encode<T: prost::Message>(msg: T) -> Result<Vec<u8>, EncodeError> {
        // Go-perun writes a u16 for the length (2 bytes), this means we cannot
        // use `encode_length_delimited`, which would write a variable length
        // integer using LEB128.
        let len = msg.encoded_len();
        // TODO: How should we handle this case? Go-perun seems to just cast to
        // uint16 and throws away the rest (no panic, no error). It is probably
        // best to return an error here, for now we're just panicking in this
        // case (as we're using unwrap below, too).
        assert!(len < (1 << 16));

        let mut buf = Vec::with_capacity(2 + len);
        buf.put_slice(&(len as u16).to_be_bytes());
        msg.encode(&mut buf)?;
        Ok(buf)
    }
}

impl<B: BytesBus> MessageBus for ProtoBufEncodingLayer<B> {
    fn send_to_watcher(&self, msg: WatcherRequestMessage) {
        let wiremsg: message::Msg = match msg {
            WatcherRequestMessage::WatchRequest(msg) => message::Msg::WatchRequest(msg.into()),
            WatcherRequestMessage::StartDispute(msg) => message::Msg::ForceCloseRequest(msg.into()),
        };
        let envelope = Message { msg: Some(wiremsg) };

        let buf = Self::encode(envelope).unwrap();
        self.bus.send_to_watcher(&buf);
    }

    fn send_to_funder(&self, msg: FunderRequestMessage) {
        let wiremsg: message::Msg = match msg {
            FunderRequestMessage::FundReq(msg) => message::Msg::FundReq(msg.into()),
            FunderRequestMessage::RegisterReq(msg) => message::Msg::RegisterReq(msg.into()),
            FunderRequestMessage::WithdrawReq(msg) => message::Msg::WithdrawReq(msg.into()),
        };
        let envelope = Message { msg: Some(wiremsg) };

        let buf = Self::encode(envelope).unwrap();
        self.bus.send_to_funder(&buf);
    }

    fn send_to_participant(
        &self,
        sender: &Identity,
        recipient: &Identity,
        msg: ParticipantMessage,
    ) {
        let wiremsg: envelope::Msg = match msg {
            ParticipantMessage::Auth => envelope::Msg::AuthResponseMsg(AuthResponseMsg {}),
            ParticipantMessage::ChannelProposal(msg) => {
                envelope::Msg::LedgerChannelProposalMsg(msg.into())
            }
            ParticipantMessage::ProposalAccepted(msg) => {
                envelope::Msg::LedgerChannelProposalAccMsg(msg.into())
            }
            ParticipantMessage::ProposalRejected { id, reason } => {
                envelope::Msg::ChannelProposalRejMsg(ChannelProposalRejMsg {
                    proposal_id: id.0.to_vec(),
                    reason,
                })
            }
            ParticipantMessage::ChannelUpdate(msg) => envelope::Msg::ChannelUpdateMsg(msg.into()),
            ParticipantMessage::ChannelUpdateAccepted(msg) => {
                envelope::Msg::ChannelUpdateAccMsg(msg.into())
            }
            ParticipantMessage::ChannelUpdateRejected {
                id,
                version,
                reason,
            } => envelope::Msg::ChannelUpdateRejMsg(ChannelUpdateRejMsg {
                channel_id: id.0.to_vec(),
                version,
                reason,
            }),
        };

        let envelope = Envelope {
            sender: sender.clone(),
            recipient: recipient.clone(),
            msg: Some(wiremsg),
        };

        let buf = Self::encode(envelope).unwrap();
        self.bus.send_to_participant(sender, recipient, &buf);
    }
}
