#![cfg_attr(not(feature = "std"), no_main)]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(not(feature = "std"), feature(default_alloc_error_handler))]

use core::cell::RefCell;
use core::option::Option::{None, Some};
use perun::channel::ActiveChannel;
use perun::{
    channel::{
        fixed_size_payment::{Allocation, Balances, ParticipantBalances},
        Asset, ChannelUpdate, LedgerChannelProposal,
    },
    perunwire::{self, envelope},
    sig::Signer,
    wire::{BytesBus, Identity, MessageBus, ProtoBufEncodingLayer},
    Address, PerunClient,
};
use prost::Message;
use rand::{CryptoRng, Rng};

#[cfg(not(any(feature = "std", feature = "nostd-example")))]
compile_error!("When running this example in no_std add the feature flag 'nostd-example'");

// Panic handler
#[cfg(not(feature = "std"))]
use panic_semihosting as _;
// use panic_halt as _;

// Global allocator
#[cfg(not(feature = "std"))]
use embedded_alloc::Heap;
#[cfg(not(feature = "std"))]
#[global_allocator]
static HEAP: Heap = Heap::empty();

// Vectors (heap allocation)
#[cfg(not(feature = "std"))]
extern crate alloc;
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

// Dependencies for running in qemu
#[cfg(not(feature = "std"))]
use cortex_m_rt::entry;
#[cfg(not(feature = "std"))]
use cortex_m_semihosting::{debug, hprint};

// Make it runnable in qemu
#[cfg(not(feature = "std"))]
#[entry]
fn entry() -> ! {
    // Initialize the allocator BEFORE you use it
    {
        use core::mem::MaybeUninit;
        const HEAP_SIZE: usize = 2048;
        static mut HEAP_MEM: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
        unsafe { HEAP.init(HEAP_MEM.as_ptr() as usize, HEAP_SIZE) }
    }

    main();

    // exit QEMU
    // NOTE do not run this on hardware; it can corrupt OpenOCD state
    debug::exit(debug::EXIT_SUCCESS);

    loop {}
}

const PARTICIPANTS: [&'static str; 2] = ["Bob", "Alice"];
const NORMAL_CLOSE: bool = false;
const SEND_DISPUTE: bool = true;

#[derive(Debug, Clone, Copy, Default)]
pub struct Config {
    pub eth_holder: Address,
    pub withdraw_receiver: Address,
}

#[cfg(not(feature = "std"))]
macro_rules! print {
    ($($arg:tt)*) => {
        hprint!($($arg)*);
    };
}

/// Helper macro to print significant places in the protocol.
macro_rules! print_bold {
    ($($arg:tt)*) => {
        print!("\x1b[1m");
        print!($($arg)*);
        print!("\x1b[0m\n");
    };
}

/// Helper macro to print points where the user/application has to interact.
macro_rules! print_user_interaction {
    ($($arg:tt)*) => {
        print!("\x1b[1;34m");
        print!($($arg)*);
        print!("\x1b[0m\n");
    };
}

#[cfg(all(feature = "std", not(feature = "no-go-comm")))]
mod net {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpStream,
    };

    pub fn read_config() -> Config {
        // Some information about the (temporary) blockchain we need, could be hard
        // coded into the application or received by some other means.
        let mut config_stream = TcpStream::connect("127.0.0.1:1339").unwrap();
        let mut buf = [0u8; 20];

        config_stream.read_exact(&mut buf).unwrap();
        let eth_holder = Address(buf);

        config_stream.read_exact(&mut buf).unwrap();
        let withdraw_receiver = Address(buf);

        Config {
            eth_holder,
            withdraw_receiver,
        }
    }

    /// Message bus representing a tcp connection. For simplicity only using
    /// [std::sync::mpsc] and printing the data to stdout.
    #[derive(Debug)]
    pub struct Bus {
        participant: usize,
        stream: RefCell<TcpStream>,
        remote_stream: RefCell<TcpStream>,
    }

    impl Bus {
        pub fn new() -> Self {
            Self {
                participant: 0,
                stream: RefCell::new(TcpStream::connect("127.0.0.1:1337").unwrap()),
                remote_stream: RefCell::new(TcpStream::connect("127.0.0.1:1338").unwrap()),
            }
        }

        pub fn recv_envelope(&self) -> perunwire::Envelope {
            Self::recv_to(&self.stream)
        }

        pub fn recv_message(&self) -> perunwire::Message {
            Self::recv_to(&self.remote_stream)
        }

        fn recv_to<T: Message + Default>(stream: &RefCell<TcpStream>) -> T {
            let buf = Self::recv(stream);

            // Decode data (the Encoding Layer currently does not decode, so to
            // print stuff or call methods we have to do it at the moment).
            let msg = T::decode(buf.as_slice()).unwrap();
            println!("Received: {:#?}", msg);

            msg
        }

        fn recv(stream: &RefCell<TcpStream>) -> Vec<u8> {
            let mut stream = stream.borrow_mut();
            // big endian u16 for length in bytes
            let mut buf = [0u8; 2];
            stream.read_exact(&mut buf).unwrap();
            let len = u16::from_be_bytes(buf);
            // Protobuf encoded data
            let mut buf = vec![0u8; len as usize];
            stream.read_exact(&mut buf).unwrap();
            buf
        }
    }

    impl BytesBus for &Bus {
        fn send_to_watcher(&self, msg: &[u8]) {
            print!("{}->Watcher: {:?}\n", PARTICIPANTS[self.participant], msg);
            self.remote_stream.borrow_mut().write(msg).unwrap();
        }

        fn send_to_funder(&self, msg: &[u8]) {
            print!("{}->Funder: {:?}\n", PARTICIPANTS[self.participant], msg);
            self.remote_stream.borrow_mut().write(msg).unwrap();
        }

        fn send_to_participant(&self, _: &Identity, _: &Identity, msg: &[u8]) {
            print!(
                "{}->{}: {:?}\n",
                PARTICIPANTS[self.participant],
                PARTICIPANTS[1 - self.participant],
                msg,
            );
            self.stream.borrow_mut().write(msg).unwrap();
        }
    }
}

#[cfg(any(not(feature = "std"), feature = "no-go-comm"))]
mod net {
    use perun::{
        abiencode::{self, types::Bytes32},
        channel::fixed_size_payment::{Params, State},
        messages::{LedgerChannelProposalAcc, LedgerChannelUpdate, LedgerChannelUpdateAccepted},
        perunwire::{message, AuthResponseMsg, Envelope},
        sig::k256::Signer,
    };
    use sha3::{Digest, Sha3_256};

    use super::*;
    use core::fmt::Debug;
    use rand::{rngs::StdRng, SeedableRng};

    pub fn read_config() -> Config {
        print!("read_config\n");
        Config::default()
    }

    #[derive(Debug)]
    struct InnerMutableData {
        send_counter: usize,
        rng: StdRng,
        signer: Signer,
        proposal: Option<LedgerChannelProposal>,
        state: Option<State<1, 2>>,
    }

    #[derive(Debug)]
    pub struct Bus {
        participant: usize,
        inner: RefCell<InnerMutableData>,
    }

    impl Bus {
        pub fn new() -> Self {
            print!("Bus::new\n");

            // Don't do that in production! For this example/demonstration this was the
            // easiest way to get a working (though deterministic) Rng.
            let mut rng = StdRng::seed_from_u64(666);
            let signer = Signer::new(&mut rng);

            let inner = InnerMutableData {
                send_counter: 0,
                rng,
                signer,
                proposal: None,
                state: None,
            };

            Self {
                participant: 0,
                inner: RefCell::new(inner),
            }
        }

        pub fn recv_envelope(&self) -> perunwire::Envelope {
            print!("Bus::recv_envelope (replying with scripted value)\n");
            let peers = get_peers();
            let mut inner = self.inner.borrow_mut();

            let wiremsg = match inner.send_counter {
                0 => envelope::Msg::AuthResponseMsg(AuthResponseMsg {}),
                1 => {
                    let nonce_share: Bytes32 = inner.rng.gen();
                    let proposal = inner
                        .proposal
                        .as_ref()
                        .expect("Example should have proposed a channel by now.");
                    let proposal_id = proposal.proposal_id;

                    let mut hasher = Sha3_256::new();
                    hasher.update(proposal.nonce_share.0);
                    hasher.update(nonce_share.0);
                    let nonce =
                        abiencode::types::U256::from_big_endian(hasher.finalize().as_slice());

                    let params = Params {
                        challenge_duration: proposal.challenge_duration,
                        nonce: nonce,
                        participants: [proposal.participant, inner.signer.address()],
                        app: Address::default(),
                        ledger_channel: true,
                        virtual_channel: false,
                    };
                    inner.state = Some(State::new(params, proposal.init_bals).unwrap());

                    envelope::Msg::LedgerChannelProposalAccMsg(
                        LedgerChannelProposalAcc {
                            nonce_share,
                            participant: inner.signer.address(),
                            proposal_id,
                        }
                        .into(),
                    )
                }
                2 => {
                    let hash = abiencode::to_hash(&inner.state.unwrap()).unwrap();
                    let sig = inner.signer.sign_eth(hash);
                    envelope::Msg::ChannelUpdateAccMsg(
                        LedgerChannelUpdateAccepted {
                            channel: inner
                                .state
                                .expect("Example should have proposed a channel by now.")
                                .channel_id(),
                            version: 0,
                            sig,
                        }
                        .into(),
                    )
                }
                3 | 4 => panic!("Expected to send message message"),
                5 => {
                    let hash = abiencode::to_hash(&inner.state.unwrap()).unwrap();
                    let sig = inner.signer.sign_eth(hash);
                    envelope::Msg::ChannelUpdateAccMsg(
                        LedgerChannelUpdateAccepted {
                            channel: inner.state.unwrap().channel_id(),
                            version: 1,
                            sig,
                        }
                        .into(),
                    )
                }
                x if x < 7 => panic!("Expected to send envelope message"),
                _ => unimplemented!("End of scripted responses"),
            };
            let response = Envelope {
                sender: peers[1].clone(),
                recipient: peers[0].clone(),
                msg: Some(wiremsg),
            };
            inner.send_counter += 1;
            response
        }

        pub fn recv_message(&self) -> perunwire::Message {
            print!("Bus::recv_message\n");
            let mut inner = self.inner.borrow_mut();
            let wiremsg = match inner.send_counter {
                3 => message::Msg::WatchResponse(perunwire::WatchResponseMsg {
                    channel_id: inner.state.unwrap().channel_id().0.to_vec(),
                    version: 0,
                    success: true,
                }),
                4 => message::Msg::FundingResponse(perunwire::FundingResponseMsg {
                    channel_id: inner.state.unwrap().channel_id().0.to_vec(),
                    success: true,
                }),
                6 => message::Msg::ForceCloseResponse(perunwire::ForceCloseResponseMsg {
                    channel_id: inner.state.unwrap().channel_id().0.to_vec(),
                    success: true,
                }),
                x if x < 7 => panic!("Expected to send envelope message"),
                _ => unimplemented!("End of scripted responses"),
            };
            inner.send_counter += 1;
            perunwire::Message { msg: Some(wiremsg) }
        }
    }

    impl BytesBus for &Bus {
        fn send_to_watcher(&self, msg: &[u8]) {
            print!("{}->Watcher: {:?}\n", PARTICIPANTS[self.participant], msg);
        }

        fn send_to_funder(&self, msg: &[u8]) {
            print!("{}->Funder: {:?}\n", PARTICIPANTS[self.participant], msg);
        }

        fn send_to_participant(&self, _: &Identity, _: &Identity, msg: &[u8]) {
            print!(
                "{}->{}: {:?}\n",
                PARTICIPANTS[self.participant],
                PARTICIPANTS[1 - self.participant],
                msg,
            );
            let mut inner = self.inner.borrow_mut();
            let envelope: perunwire::Envelope = perunwire::Envelope::decode(&msg[2..]).unwrap();
            match envelope.msg.unwrap() {
                perunwire::envelope::Msg::LedgerChannelProposalMsg(msg) => {
                    inner.proposal = Some(msg.try_into().unwrap())
                }
                perunwire::envelope::Msg::ChannelUpdateMsg(msg) => {
                    let update: LedgerChannelUpdate = msg.try_into().unwrap();
                    inner.state = Some(update.state);
                }
                _ => {}
            }
        }
    }
}

use net::Bus;

#[cfg(feature = "std")]
fn get_rng() -> impl Rng + CryptoRng {
    rand::thread_rng()
}
#[cfg(not(feature = "std"))]
fn get_rng() -> impl Rng + CryptoRng {
    use rand::SeedableRng;

    // Don't do that in production! For this example/demonstration this was the
    // easiest way to get a working (though deterministic) Rng.
    rand::rngs::StdRng::seed_from_u64(0)
}

fn get_peers() -> Vec<Vec<u8>> {
    PARTICIPANTS.map(|p| p.as_bytes().to_vec()).into()
}

fn main() {
    let mut rng = get_rng();

    // Some information about the (temporary) blockchain we need, could be hard
    // coded into the application or received by some other means.
    let config = net::read_config();

    // Networking
    let bus = Bus::new();
    let peers = get_peers();

    // Signer, Addresses and Client
    let signer = Signer::new(&mut rng);
    let addr = signer.address();
    let client = PerunClient::new(ProtoBufEncodingLayer { bus: &bus }, signer);
    client.send_handshake_msg(&peers[0], &peers[1]);
    bus.recv_envelope();

    // Create channel proposal (user configuration)
    print_user_interaction!("Proposing channel");
    let init_balance = Balances([ParticipantBalances([100.into(), 100.into()])]);
    let prop = LedgerChannelProposal {
        proposal_id: rng.gen(),
        challenge_duration: 25,
        nonce_share: rng.gen(),
        init_bals: Allocation::new(
            [Asset {
                holder: config.eth_holder,
            }],
            init_balance,
        ),
        funding_agreement: init_balance,
        participant: addr,
        peers,
    };
    // Propose new channel and wait for responses
    let mut channel = client
        .propose_channel(prop, config.withdraw_receiver)
        .unwrap();
    match bus.recv_envelope().msg {
        Some(envelope::Msg::LedgerChannelProposalAccMsg(msg)) => channel
            .participant_accepted(1, msg.try_into().unwrap())
            .unwrap(),
        Some(envelope::Msg::ChannelProposalRejMsg(_)) => {
            print_bold!("Bob done: Received ProposalRejected");
            return;
        }
        Some(_) => panic!("Unexpected message"),
        None => panic!("Envelope did not contain a msg"),
    }
    print_bold!(
        "Both agreed on proposal and nonces, Both sign the initial state and exchange signatures"
    );

    // Go to Phase 2: Signing the initial state
    let mut channel = channel.build().unwrap();
    channel.sign().unwrap();
    match bus.recv_envelope().msg {
        Some(envelope::Msg::ChannelUpdateAccMsg(msg)) => {
            channel.add_signature(msg.try_into().unwrap()).unwrap()
        }
        Some(envelope::Msg::ChannelUpdateRejMsg(_)) => {
            print_bold!("Bob done: Did not receive Signature from Bob");
            return;
        }
        Some(_) => panic!("Unexpected message"),
        None => panic!("Envelope did not contain a msg"),
    }

    print_bold!("Bob: Received all signatures, send to watcher/funder");

    let channel = channel.build().unwrap();
    // Receive acknowledgements (currently not checked but we have to read them
    // anyways).
    bus.recv_message();
    bus.recv_message();

    let mut channel = channel.mark_funded();

    print_user_interaction!("Bob: Propose Update");
    let mut new_state = channel.state().make_next_state();
    new_state.outcome.balances.0[0].0[0] += 10.into();
    new_state.outcome.balances.0[0].0[1] -= 10.into();
    let update = channel.update(new_state).unwrap();
    handle_update_response(&bus, &mut channel, update);

    if NORMAL_CLOSE {
        print_user_interaction!("Bob: Propose Normal close");
        let mut new_state = channel.state().make_next_state();
        // Propose a normal closure
        new_state.is_final = true;
        let update = channel.update(new_state).unwrap();
        handle_update_response(&bus, &mut channel, update);
    }

    if SEND_DISPUTE {
        print_user_interaction!("Bob: Send StartDispute Message (force-close)");
        channel.force_close().unwrap();
        bus.recv_message();
    }

    print_bold!("Bob done");
}

fn handle_update_response(
    bus: &Bus,
    channel: &mut ActiveChannel<impl MessageBus>,
    mut update: ChannelUpdate,
) {
    match bus.recv_envelope().msg {
        Some(envelope::Msg::ChannelUpdateAccMsg(msg)) => {
            update
                .participant_accepted(channel, 1, msg.try_into().unwrap())
                .unwrap();
            update.apply(channel).unwrap();
        }
        Some(envelope::Msg::ChannelUpdateRejMsg(_)) => {
            print_bold!("Aborting update");
            drop(update);
        }
        Some(_) => panic!("Unexpected message"),
        None => panic!("Envelope did not contain a msg"),
    }
}
