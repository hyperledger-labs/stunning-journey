//! State machine for the application logic. We need to do networking during the
//! setup and application logic, so we need to do it while the main loop is
//! running. Additionally, some steps cannot finish immediately. Since we
//! (currently at least) don't have an async runtime in this demo the easiest
//! way to do this is to have a state machine for the setup and application
//! logic, too, which is contained in this module.

use core::cell::RefCell;

use alloc::vec::Vec;
use perun::{
    abiencode::types::U256,
    channel::{
        fixed_size_payment::{Allocation, Balances, ParticipantBalances},
        Asset, ProposalBuildError, SignError,
    },
    messages::{
        ConversionError, FunderReplyMessage, LedgerChannelProposal, ParticipantMessage,
        WatcherReplyMessage,
    },
    perunwire::{
        self,
        envelope::{self, Msg},
        Envelope,
    },
    wire::ProtoBufEncodingLayer,
    Address, Hash, InvalidProposal, PerunClient,
};
use prost::{DecodeError, Message};
use rand::{rngs::StdRng, Rng};
use rand_core::RngCore;
use smoltcp::{
    iface::{Interface, SocketHandle},
    phy::Device,
    socket::TcpSocket,
    wire::IpAddress,
};

use crate::{
    bus::Bus,
    channel::{self, Channel},
};

/// We are currently copying from the rx-buffer to a slice for decoding
/// protobuf, because that needs a single consecutive area of memory (see
/// comments in [`try_recv`] for details).
pub const MAX_MESSAGE_SIZE: usize = 510;

/// Configuration for the demo: Peers and where to find the
/// participant/watcher/funder.
pub struct Config {
    pub config_server: (IpAddress, u16),
    pub other_participant: (IpAddress, u16),
    pub service_server: (IpAddress, u16),
    pub listen_port: u16,
    pub participants: [&'static str; 2],
}

/// State machine for the demo logic: Fetch information about the blockchain
/// from the go-side, create TCP socket with participant and propose channel.
pub struct Application<'cl, DeviceT>
where
    DeviceT: for<'d> Device<'d>,
{
    state: ApplicationState<'cl, DeviceT>,
    iface: &'cl RefCell<Interface<'cl, DeviceT>>,
    participant_handle: SocketHandle,
    service_handle: SocketHandle,
    config: Config,
    rng: StdRng,
    client: &'cl PerunClient<ProtoBufEncodingLayer<Bus<'cl, DeviceT>>>,
    addr: Address,
}

/// Enum to represent the states the Application can be in.
enum ApplicationState<'cl, DeviceT>
where
    DeviceT: for<'d> Device<'d>,
{
    /// Initial state, nothing has been done yet, the application was just
    /// started. Immediately transition to `ConnectingToConfigDealer`
    InitialState,
    /// Setting up the TCP connection to get info about the blockchain this demo
    /// is using (eth-holder and withdraw_receiver). As soon as the connection is
    /// established we read from it and go to `ClosingSockets`.
    ConnectingToConfigDealer,
    /// We have everything we need, wait until the setup connection is closed,
    /// then setup TCP listening and transition to `Listening`
    ClosingSockets {
        eth_holder: Address,
        withdraw_receiver: Address,
    },
    /// Wait and do nothing until someone presses a button or we receive a tcp
    /// connection attempt, then transition into `Connecting` or
    /// `WaitForProposal` respectively. In both cases connect to the
    /// funder/watcher.
    Listening {
        eth_holder: Address,
        withdraw_receiver: Address,
    },
    /// We have received a connection and gotten a handshake (and sent a
    /// response handshake). Wait until we have connected to the watcher/funder
    /// and receive a channel proposal, then accept it and transition into
    /// `Active`.
    WaitForProposal {
        eth_holder: Address,
        withdraw_receiver: Address,
    },
    /// Setting up the TCP connections to other participant (p2p) and remote
    /// funder/watcher. Once the connections are both established send the
    /// handshake message and transition to `WaitForHandshake`.
    Connecting {
        eth_holder: Address,
        withdraw_receiver: Address,
    },
    /// Wait until we receive the handshake response, then propose a channel and
    /// transition to `Active`.
    WaitForHandshake {
        eth_holder: Address,
        withdraw_receiver: Address,
    },
    /// We have an open channel, the logic of which is handled in a separate
    /// state machine. If the channel closes transition to
    /// `ClosingSockets`.
    Active {
        eth_holder: Address,
        withdraw_receiver: Address,
        channel: Channel<'cl, ProtoBufEncodingLayer<Bus<'cl, DeviceT>>>,
    },
}

#[derive(Debug)]
pub enum Error {
    Network(smoltcp::Error),
    InvalidProposal(InvalidProposal),
    ProstDecode(DecodeError),
    EnvelopeHasNoMsg,
    UnexpectedMsg,
    ConversionError(ConversionError),
    ChannelError(channel::Error),
    InvalidState,
    MessageLargerThanRxBuffer(usize),
    ProposalBuildError(ProposalBuildError),
    SignError(SignError),
}

impl From<smoltcp::Error> for Error {
    fn from(e: smoltcp::Error) -> Self {
        Self::Network(e)
    }
}
impl From<InvalidProposal> for Error {
    fn from(e: InvalidProposal) -> Self {
        Self::InvalidProposal(e)
    }
}
impl From<prost::DecodeError> for Error {
    fn from(e: prost::DecodeError) -> Self {
        Self::ProstDecode(e)
    }
}
impl From<ConversionError> for Error {
    fn from(e: ConversionError) -> Self {
        Self::ConversionError(e)
    }
}
impl From<channel::Error> for Error {
    fn from(e: channel::Error) -> Self {
        Self::ChannelError(e)
    }
}
impl From<ProposalBuildError> for Error {
    fn from(e: ProposalBuildError) -> Self {
        Self::ProposalBuildError(e)
    }
}
impl From<SignError> for Error {
    fn from(e: SignError) -> Self {
        Self::SignError(e)
    }
}

enum ServiceReplyMessage {
    Watcher(WatcherReplyMessage),
    Funder(FunderReplyMessage),
}

impl<'cl, DeviceT> Application<'cl, DeviceT>
where
    DeviceT: for<'d> Device<'d>,
{
    pub fn new(
        participant_handle: SocketHandle,
        service_handle: SocketHandle,
        config: Config,
        rng: StdRng,
        addr: Address,
        client: &'cl PerunClient<ProtoBufEncodingLayer<Bus<'cl, DeviceT>>>,
        iface: &'cl RefCell<Interface<'cl, DeviceT>>,
    ) -> Self {
        Self {
            state: ApplicationState::InitialState,
            participant_handle,
            service_handle,
            config,
            rng,
            client,
            addr,
            iface,
        }
    }

    fn connect_config_dealer(&mut self) -> Result<(), Error> {
        let mut iface = self.iface.borrow_mut();
        let (socket, cx) = iface.get_socket_and_context::<TcpSocket>(self.participant_handle);
        socket.connect(
            cx,
            self.config.config_server,
            (IpAddress::Unspecified, self.get_ethemeral_port()),
        )?;

        self.state = ApplicationState::ConnectingToConfigDealer;
        Ok(())
    }

    fn wait_connected_and_read_config(&mut self) -> Result<(), Error> {
        let mut iface = self.iface.borrow_mut();
        let socket = iface.get_socket::<TcpSocket>(self.participant_handle);
        if socket.is_active() && socket.can_recv() {
            // Try reading from the socket. Returns Err if there is something
            // wrong with the socket (unexpected tcp state). Returns None if not
            // enough bytes are available (we only received partial data for
            // some reason).
            //
            // Note that this will fail if we are at a ringbuffer boundry, see
            // `try_recv` for details. In this demo this is not a problem
            // because the rx_buffer is always empty when this function is
            // called and can thus always fit 40 bytes in a consecutive slice.
            if let Some((eth_holder, withdraw_receiver)) = socket.recv(|x| {
                if x.len() >= 40 {
                    let eth_holder = Address(x[..20].try_into().unwrap());
                    let withdraw_receiver = Address(x[20..40].try_into().unwrap());
                    (40, Some((eth_holder, withdraw_receiver)))
                } else {
                    (0, None)
                }
            })? {
                self.state = ApplicationState::ClosingSockets {
                    eth_holder,
                    withdraw_receiver,
                };
                socket.close();
            }
        }
        Ok(())
    }

    fn wait_connections_closed(
        &mut self,
        eth_holder: Address,
        withdraw_receiver: Address,
    ) -> Result<(), Error> {
        // Only continue if the sockets are free (i.e. closed) and avaliable.
        // Alternatively we could use `socket.abort()`, resulting in a
        // non-graceful shutdown but slightly faster transition times. One
        // downside of doing it this way is that a malicious config dealer could
        // DoS us by never sending a Fin, but since the config dealer is only
        // necessary for the demo (which can't use hard-coded addresses) this is
        // not a problem.
        //
        // We have to get the socket multiple times because of the lifetimes in
        // `iface.get_socket` and we can only start the connection if both
        // sockets are free.
        let mut iface = self.iface.borrow_mut();
        let ssocket_active = iface
            .get_socket::<TcpSocket>(self.service_handle)
            .is_active();
        let psocket = iface.get_socket::<TcpSocket>(self.participant_handle);
        if !ssocket_active && !psocket.is_active() {
            psocket.listen(self.config.listen_port)?;
            self.state = ApplicationState::Listening {
                eth_holder,
                withdraw_receiver,
            };
        }
        Ok(())
    }

    fn check_incomming_connection(
        &mut self,
        eth_holder: Address,
        withdraw_receiver: Address,
    ) -> Result<(), Error> {
        // Scope iface because `try_recv_participant_msg` needs to borrow it, too.
        {
            let mut iface = self.iface.borrow_mut();
            let psocket = iface.get_socket::<TcpSocket>(self.participant_handle);
            if !psocket.is_open() || !psocket.may_recv() {
                // We don't have a connection, yet
                return Ok(());
            }
        }

        let env: Envelope = match self.try_recv(self.participant_handle)? {
            Some(env) => env,
            None => return Ok(()),
        };

        match env.msg {
            Some(envelope::Msg::AuthResponseMsg(_)) => {}
            Some(_) => return Err(Error::UnexpectedMsg),
            None => return Err(Error::InvalidState),
        }

        let my_wire_address = self.config.participants[0].into();

        if env.recipient[..] != self.config.participants[0].as_bytes()[..] {
            return Err(Error::UnexpectedMsg);
        }

        self.client
            .send_handshake_msg(&my_wire_address, &env.sender);

        let mut iface = self.iface.borrow_mut();
        let (ssocket, cx) = iface.get_socket_and_context::<TcpSocket>(self.service_handle);
        ssocket.connect(
            cx,
            self.config.service_server,
            (IpAddress::Unspecified, self.get_ethemeral_port()),
        )?;

        self.state = ApplicationState::WaitForProposal {
            eth_holder,
            withdraw_receiver,
        };
        Ok(())
    }

    fn wait_connected_and_proposal_msg(
        &mut self,
        eth_holder: Address,
        withdraw_receiver: Address,
    ) -> Result<(), Error> {
        {
            let mut iface = self.iface.borrow_mut();
            let ssocket = iface.get_socket::<TcpSocket>(self.service_handle);
            if !ssocket.is_open() {
                // We don't have a connection, yet
                return Ok(());
            }
        }

        match self.try_recv_participant_msg()? {
            Some(ParticipantMessage::ChannelProposal(prop)) => {
                let mut channel = self.client.handle_proposal(prop, withdraw_receiver)?;
                // Accept the proposal (this can not fail because we only just
                // got the channel and so we cannot have accepted it already).
                channel.accept(self.rng.gen(), self.addr).unwrap();
                // Immediately process into an agreed upon channel (because we
                // have just accepted it and this state machine only supports
                // 2-participant channels).
                let mut channel = channel.build().map_err(|(_, e)| e)?;
                // We also have to sign it.
                channel.sign()?;
                self.state = ApplicationState::Active {
                    eth_holder,
                    withdraw_receiver,
                    channel: Channel::new_agreed_upon(channel),
                };
                Ok(())
            }
            Some(_) => Err(Error::InvalidState),
            None => Ok(()),
        }
    }

    /// Connect to both participant and watcher/funder, then propose a channel
    /// in a later state.
    fn connect(&mut self, eth_holder: Address, withdraw_receiver: Address) -> Result<(), Error> {
        let mut iface = self.iface.borrow_mut();

        let (psocket, cx) = iface.get_socket_and_context::<TcpSocket>(self.participant_handle);
        // Stop the TCP listener, we need the socket ourselves and cannot have
        // multiple active channels anyways.
        if psocket.is_listening() {
            psocket.abort();
        }
        psocket.connect(
            cx,
            self.config.other_participant,
            (IpAddress::Unspecified, self.get_ethemeral_port()),
        )?;

        let (ssocket, cx) = iface.get_socket_and_context::<TcpSocket>(self.service_handle);
        ssocket.connect(
            cx,
            self.config.service_server,
            (IpAddress::Unspecified, self.get_ethemeral_port()),
        )?;

        self.state = ApplicationState::Connecting {
            eth_holder,
            withdraw_receiver,
        };
        Ok(())
    }

    fn wait_connected_and_send_handshake(
        &mut self,
        eth_holder: Address,
        withdraw_receiver: Address,
    ) -> Result<(), Error> {
        let mut iface = self.iface.borrow_mut();

        // Wait for the participant socket and send handshake (only transition
        // if both are ready)
        let psocket = iface.get_socket::<TcpSocket>(self.participant_handle);
        if psocket.is_active() && psocket.may_recv() && psocket.may_send() {
            // propose_channel neeeds to be able to borrow the interface to send
            // things on the network. Because of this we need to drop the
            // interface first. Alternatively we could have moved
            // propose_channel to a new state or restructured this function to
            // automatically drop it before calling propose_channel.
            drop(psocket);
            drop(iface);

            // Handshake
            let peers: Vec<Vec<u8>> = self
                .config
                .participants
                .map(|p| p.as_bytes().to_vec())
                .into();
            self.client.send_handshake_msg(&peers[0], &peers[1]);

            self.state = ApplicationState::WaitForHandshake {
                eth_holder,
                withdraw_receiver,
            }
        }
        Ok(())
    }

    fn try_recv<T: Message + Default>(&mut self, handle: SocketHandle) -> Result<Option<T>, Error> {
        // Yes, this function is long when including comments. When not
        // including them it is still complex, but I have not found a way to do
        // this without reading everything into a heap-allocated buffer or
        // storing some information between calls to try_recv using the API
        // smoltcp currently provides.
        let mut iface = self.iface.borrow_mut();
        let socket = iface.get_socket::<TcpSocket>(handle);

        let recv_queue = socket.recv_queue();
        if recv_queue < 2 {
            return Ok(None); // We don't have 2 bytes of length
        }

        // Peek at the message length (keeping length and message in the
        // rx-buffer if it is not completely received)
        let mut buf_msg_length = [0u8; 2];
        let bytes_peeked = socket.peek_slice(&mut buf_msg_length)?;
        if bytes_peeked < 2 {
            // smoltcp currently does not provide the capability to peek
            // over the edge of the (internal) rx ringbuffer. the current
            // peek cannot have this ability without copying data
            // internally, peek_slice does however, at least based on its
            // API design and comment. Unfortunately (likely due to a bug in
            // smoltcp) it does not do so, which makes it impossible to read
            // the length if we are at the end of the ringbuffer. This can
            // be solved in one of the following ways:
            // - Change peek_slice to do what the comment says: Do the same
            //   as recv_slice, which does look over the ringbuffer boundry.
            // - Add a `peek_offset(&mut self, size: usize, offset: usize)`
            //   to smoltcp which allows us to do option 1 ourselves
            // - Read and dequeue the message length, then store it
            //   somewhere in the application.
            // - Read and dequeue the message length, then immediately
            //   follow with the message and panic if it is not complete,
            //   yet. This would likely happen more often than panicing if
            //   we are exactly at the ringbuffer border.
            //
            // Technical debt: Because this is likely a bug in smoltcp and
            // option 3 would require a lot of changes we're panicking in
            // this case for now (at least until we have this fixed in a
            // separate branch on smoltcp or a fork).
            //
            // The probability that this happens is `1/rx_buffer.len()`,
            // which is currently < 1/512.
            panic!("Bug/Limitation in smoltcp");
        }
        let length: usize = u16::from_be_bytes(buf_msg_length).into();

        // Make sure it is even possible to receive the message.
        if (2 + length) > socket.recv_capacity() {
            // To handle messages larger than the rx_buffer size requires one of
            // the following:
            // - Partial protobuf decoding and storing the partial data
            //   somewhere. Difficult if not impossible with the Protobuf
            //   library (although it should in theory be possible)
            // - Copying the data into a separate buffer that can hold it over
            //   multiple poll calls. Difficult to do, especially since that
            //   would require a heap large enough to store the data which could
            //   be up to 64KiB of space, which would be near impossible on a
            //   device with just low ram.
            // - Keep a counter of the remaining message size and discard a
            //   message over multiple calls to `try_recv` (with calls to
            //   `iface.poll` in between). This would allow keeping the
            //   connection open even if someone sends a too big message. The
            //   problem with this approach is that it may break some
            //   assumptions on the other side.
            // - Panic or return an error, thus effectively dropping the
            //   connection as there is no way to handle such big messages. This
            //   is the option implemented below.
            //
            // Note that such messages won't happen under normal protocol
            // completion as long as the rx_buffer is large enough to hold the
            // largest possible message type (512 is sufficient for channels
            // with 2 participants and 1 asset).
            return Err(Error::MessageLargerThanRxBuffer(2 + length));
        }

        // Only continue if the message is complete.
        if socket.recv_queue() < 2 + length {
            return Ok(None); // We don't have all the data
        }

        // Read the entire message and decode it.
        //
        // Technical debt: We're currently creating a copy of the bytes in
        // memory for decoding. It should be possible to do this without
        // creating a copy (in a local variable) by implementing a custom buffer
        // to decode from. This would also eliminate the need for the
        // MAX_MESSAGE_SIZE local array.
        //
        // unsized local variables are currently unstable rust, see
        // https://doc.rust-lang.org/unstable-book/language-features/unsized-locals.html.
        // Therefore we need to specify a size. We cannot take it from socket or
        // self.config because neither is constant => MAX_MESSAGE_SIZE
        //
        // Discard 2 bytes of length information.
        let read = socket.recv(|x| {
            let len = x.len().min(2);
            (len, len)
        })?;
        if read != 2 {
            // At the moment this cannot happen because we're panicking earlier
            // if we are at the bingbuffer boundry (the only situation where
            // this could happen). I've nevertheless added the logic to handle
            // this case as a defensive mechanism (i.e. we won't panic here) in
            // case someone fixes the panic above but doesn't change this part.
            socket.recv(|_| (2 - read, ()))?;
        }
        let mut buf = [0u8; MAX_MESSAGE_SIZE];
        let bytes_read = socket.recv_slice(&mut buf[..length])?;
        if bytes_read != length {
            // This can only happen if the rx_buffer runs out, which can't
            // happen because we have queued bytes. Note that this only holds
            // true as long as smoltcp does not queue out-of-order packets.
            unreachable!("We previously checked for queue size, did smoltcp add storage for out-of-order packets?")
        }
        let env = T::decode(&buf[..length])?;
        Ok(Some(env))
    }

    fn wait_handshake_and_propose_channel(
        &mut self,
        eth_holder: Address,
        withdraw_receiver: Address,
    ) -> Result<(), Error> {
        // Only continue if we have a complete package and there was no decoding
        // error. Note that we currently do not check the addresses in the
        // envelope.
        match self.try_recv_participant_msg()? {
            Some(ParticipantMessage::Auth) => {
                self.send_channel_proposal(eth_holder, withdraw_receiver)
            }
            Some(_) => Err(Error::UnexpectedMsg),
            None => Ok(()),
        }
    }

    fn send_channel_proposal(
        &mut self,
        eth_holder: Address,
        withdraw_receiver: Address,
    ) -> Result<(), Error> {
        // Channel Proposal
        let init_balance = Balances([ParticipantBalances([100_000.into(), 100_000.into()])]);
        let peers = self
            .config
            .participants
            .map(|p| p.as_bytes().to_vec())
            .into();
        let prop = LedgerChannelProposal {
            proposal_id: self.rng.gen(),
            challenge_duration: 25,
            nonce_share: self.rng.gen(),
            init_bals: Allocation::new(
                [Asset {
                    holder: eth_holder,
                }],
                init_balance,
            ),
            funding_agreement: init_balance,
            participant: self.addr,
            peers,
        };
        let channel = self.client.propose_channel(prop, withdraw_receiver)?;
        // Setup sub-state-machine for handling the channel
        let channel = Channel::new(channel);
        self.state = ApplicationState::Active {
            channel,
            eth_holder,
            withdraw_receiver,
        };
        Ok(())
    }

    fn try_recv_participant_msg(&mut self) -> Result<Option<ParticipantMessage>, Error> {
        let env: Envelope = match self.try_recv(self.participant_handle)? {
            Some(env) => env,
            None => return Ok(None),
        };
        let msg = match env.msg {
            Some(m) => m,
            None => return Err(Error::EnvelopeHasNoMsg),
        };
        let msg = match msg {
            Msg::PingMsg(_) => unimplemented!(),
            Msg::PongMsg(_) => unimplemented!(),
            Msg::ShutdownMsg(_) => unimplemented!(),
            Msg::AuthResponseMsg(_) => ParticipantMessage::Auth,
            Msg::LedgerChannelProposalMsg(m) => ParticipantMessage::ChannelProposal(m.try_into()?), // Possible in the library but this Application does not support incoming requests.
            Msg::LedgerChannelProposalAccMsg(m) => {
                ParticipantMessage::ProposalAccepted(m.try_into()?)
            }
            Msg::SubChannelProposalMsg(_) => unimplemented!(),
            Msg::SubChannelProposalAccMsg(_) => unimplemented!(),
            Msg::VirtualChannelProposalMsg(_) => unimplemented!(),
            Msg::VirtualChannelProposalAccMsg(_) => unimplemented!(),
            Msg::ChannelProposalRejMsg(m) => ParticipantMessage::ProposalRejected {
                id: Hash(m.proposal_id.try_into().unwrap()),
                reason: m.reason,
            },
            Msg::ChannelUpdateMsg(m) => ParticipantMessage::ChannelUpdate(m.try_into()?),
            Msg::VirtualChannelFundingProposalMsg(_) => unimplemented!(),
            Msg::VirtualChannelSettlementProposalMsg(_) => unimplemented!(),
            Msg::ChannelUpdateAccMsg(m) => ParticipantMessage::ChannelUpdateAccepted(m.try_into()?),
            Msg::ChannelUpdateRejMsg(m) => ParticipantMessage::ChannelUpdateRejected {
                id: Hash(m.channel_id.try_into().unwrap()),
                version: m.version,
                reason: m.reason,
            },
            Msg::ChannelSyncMsg(_) => unimplemented!(),
        };
        Ok(Some(msg))
    }

    fn try_recv_service_msg(&mut self) -> Result<Option<ServiceReplyMessage>, Error> {
        let env: perunwire::Message = match self.try_recv(self.service_handle)? {
            Some(env) => env,
            None => return Ok(None),
        };
        let msg = match env.msg {
            Some(m) => m,
            None => return Err(Error::EnvelopeHasNoMsg),
        };
        let msg = match msg {
            perunwire::message::Msg::FundingRequest(_) => unimplemented!(),
            perunwire::message::Msg::FundingResponse(m) => {
                ServiceReplyMessage::Funder(FunderReplyMessage::Funded {
                    id: Hash(m.channel_id.try_into().unwrap()),
                })
            }
            perunwire::message::Msg::WatchRequest(_) => unimplemented!(),
            perunwire::message::Msg::WatchResponse(m) => {
                ServiceReplyMessage::Watcher(WatcherReplyMessage::Ack {
                    id: Hash(m.channel_id.try_into().unwrap()),
                    version: m.version,
                })
            }
            perunwire::message::Msg::ForceCloseRequest(_) => unimplemented!(),
            perunwire::message::Msg::ForceCloseResponse(m) => {
                ServiceReplyMessage::Watcher(WatcherReplyMessage::DisputeAck {
                    id: Hash(m.channel_id.try_into().unwrap()),
                })
            }
            perunwire::message::Msg::DisputeNotification(m) => {
                ServiceReplyMessage::Watcher(WatcherReplyMessage::DisputeNotification {
                    id: Hash(m.channel_id.try_into().unwrap()),
                })
            }
        };

        Ok(Some(msg))
    }

    /// Helper function to not duplicate code. We have to process a message
    /// before we can continue with the second one, otherwise we might loose a
    /// message. The same goes for checking if the channel was closed.
    fn forward_messages<T, F1, F2>(&mut self, recv_fn: F1, process_fn: F2) -> Result<bool, Error>
    where
        F1: Fn(&mut Self) -> Result<Option<T>, Error>,
        F2: Fn(&mut Channel<ProtoBufEncodingLayer<Bus<DeviceT>>>, T) -> Result<(), Error>,
    {
        let msg: Option<T> = recv_fn(self)?;

        if let Some(msg) = msg {
            // Now get the (mutable) channel object so we don't get issues with mutability.
            let (channel, eth_holder, withdraw_receiver) = match self.state {
                ApplicationState::Active {
                    ref mut channel,
                    eth_holder,
                    withdraw_receiver,
                } => (channel, eth_holder, withdraw_receiver),
                _ => unreachable!("This function is only called when in Active"),
            };

            process_fn(channel, msg)?;

            if channel.is_closed() {
                let mut iface = self.iface.borrow_mut();
                iface
                    .get_socket::<TcpSocket>(self.participant_handle)
                    .close();
                iface.get_socket::<TcpSocket>(self.service_handle).close();
                self.state = ApplicationState::ClosingSockets {
                    eth_holder,
                    withdraw_receiver,
                };
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn forward_messages_to_channel(&mut self) -> Result<(), Error> {
        let has_participant_msg = self.forward_messages(
            |s| s.try_recv_participant_msg(),
            |ch, msg| {
                ch.process_participant_msg(msg)?;
                Ok(())
            },
        )?;

        // Only process one message, as that could have changed the channels
        // state to Closed, which would mean we don't forward messages anymore.
        // I don't know if that is really necessary but it has the additional
        // benefit of allowing Interface.poll calls in between.
        if has_participant_msg {
            return Ok(());
        }

        self.forward_messages(
            |s| s.try_recv_service_msg(),
            |ch, msg| {
                match msg {
                    ServiceReplyMessage::Funder(msg) => ch.process_funder_reply(msg)?,
                    ServiceReplyMessage::Watcher(msg) => ch.process_watcher_reply(msg)?,
                };
                Ok(())
            },
        )?;
        Ok(())
    }

    /// Main polling function transitioning between states. Call this regularly,
    /// for example always after polling the network interface.
    pub fn poll(&mut self) -> Result<(), Error> {
        match self.state {
            ApplicationState::InitialState => self.connect_config_dealer(),
            ApplicationState::ConnectingToConfigDealer => self.wait_connected_and_read_config(),
            ApplicationState::ClosingSockets {
                eth_holder,
                withdraw_receiver,
            } => self.wait_connections_closed(eth_holder, withdraw_receiver),
            ApplicationState::Listening {
                eth_holder,
                withdraw_receiver,
            } => self.check_incomming_connection(eth_holder, withdraw_receiver),
            ApplicationState::WaitForProposal {
                eth_holder,
                withdraw_receiver,
            } => self.wait_connected_and_proposal_msg(eth_holder, withdraw_receiver),
            ApplicationState::Connecting {
                eth_holder,
                withdraw_receiver,
            } => self.wait_connected_and_send_handshake(eth_holder, withdraw_receiver),
            ApplicationState::WaitForHandshake {
                eth_holder,
                withdraw_receiver,
            } => self.wait_handshake_and_propose_channel(eth_holder, withdraw_receiver),
            ApplicationState::Active { .. } => self.forward_messages_to_channel(),
        }
    }

    /// Send 100 WEI to the other channel participant to demonstrate channel
    /// updates. If the channel is not currently active it will return an error.
    pub fn update(&mut self, amount: U256, is_final: bool) -> Result<(), Error> {
        match &mut self.state {
            ApplicationState::Active { channel, .. } => {
                channel.update(amount, is_final)?;
                Ok(())
            }
            _ => Err(Error::InvalidState),
        }
    }

    /// Force close the channel by sending a DisputeRequest to the Watcher.
    pub fn force_close(&mut self) -> Result<(), Error> {
        match &mut self.state {
            ApplicationState::Active { channel, .. } => {
                channel.force_close()?;
                Ok(())
            }
            _ => Err(Error::InvalidState),
        }
    }

    /// Propose a new channel to the other participant.
    pub fn propose_channel(&mut self) -> Result<(), Error> {
        match self.state {
            ApplicationState::Listening {
                eth_holder,
                withdraw_receiver,
            } => self.connect(eth_holder, withdraw_receiver),
            _ => Err(Error::InvalidState),
        }
    }

    fn get_ethemeral_port(&mut self) -> u16 {
        const MIN: u16 = 49152;
        const MAX: u16 = 65535;
        // Note: This is not evenly distributed but sufficient for what we need.
        MIN + (self.rng.next_u32() as u16) % (MAX - MIN)
    }
}
