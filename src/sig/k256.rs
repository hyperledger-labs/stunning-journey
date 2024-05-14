//! Signer using the k256 Rust crate (implementation of ecdsa in Rust).

use crate::abiencode::types::{Address, Hash, Signature};
use k256::{
    ecdsa::{
        recoverable,
        signature::{hazmat::PrehashSigner, Signature as k256Signature},
        SigningKey, VerifyingKey,
    },
    elliptic_curve::sec1::ToEncodedPoint,
};
use sha3::{Digest, Keccak256};

use super::hash_to_eth_signed_msg_hash;

pub use k256::ecdsa::Error;

#[derive(Debug)]
pub struct Signer {
    key: SigningKey,
    addr: Address,
}

impl From<VerifyingKey> for Address {
    fn from(key: VerifyingKey) -> Self {
        // Convert the key into an EncodedPoint (on the curve), which has the
        // data we need in bytes [1..]. Then convert that into an array and
        // unwrap. This panics if the bytes representation of EncodedPoint is
        // not 65 bytes, which is unlikely to change in the dependency. If it
        // does we have bigger problems, given that its contents/layout will
        // likely change, too if the length changes.
        let pk_bytes: [u8; 65] = key.to_encoded_point(false).as_bytes().try_into().unwrap();

        // See https://ethereum.stackexchange.com/questions/65233/goethereum-getting-public-key-from-private-key-hex-formatting
        //
        // Throw away the first byte, which is not part of the public key. It is
        // added by serialize_uncompressed due to the encoding used.
        let hash: [u8; 32] = Keccak256::digest(&pk_bytes[1..]).into();

        let mut addr = Address([0; 20]);
        addr.0.copy_from_slice(&hash[32 - 20..]);
        addr
    }
}

impl Signer {
    pub fn new<R: rand::Rng + rand::CryptoRng>(rng: &mut R) -> Self {
        let private_key_bytes: [u8; 32] = [
            0x24, 0x4F, 0xFC, 0x73, 0xC4, 0x48, 0xB5, 0x6D, 0xDB, 0xA6, 0xA7, 0xBF, 0xA8, 0xD5,
            0x8E, 0xD3, 0x60, 0x12, 0x61, 0x1D, 0xA8, 0x3D, 0x4C, 0xB8, 0x30, 0x25, 0xEA, 0x12,
            0xAC, 0xCF, 0x49, 0xFE,
        ];

        let key = SigningKey::from_bytes(&private_key_bytes).expect("Invalid private key");

        let addr = key.verifying_key().into();

        Self { key, addr }
    }

    pub fn address(&self) -> Address {
        self.addr
    }

    pub fn sign_eth(&self, msg: Hash) -> Signature {
        // "\x19Ethereum Signed Message:\n32" format
        let hash = hash_to_eth_signed_msg_hash(msg);

        let sig: recoverable::Signature = self.key.sign_prehash(&hash.0).unwrap();

        // Luckily for us, this Signature type already has the format we need:
        // - 65 bytes containing r, s and v in this order
        //
        // But we still have to add 27 to v for the signature to be valid in the
        // EVM.
        let mut sig_bytes: [u8; 65] = sig.as_bytes().try_into().expect(
            "Unreachable: Signature size doesn't match, something big must have changed in the dependency",
        );
        debug_assert!(sig_bytes[32] & 0x80 == 0);
        sig_bytes[64] += 27;

        Signature(sig_bytes)
    }

    pub fn recover_signer(&self, msg: Hash, eth_sig: Signature) -> Result<Address, Error> {
        // "\x19Ethereum Signed Message:\n32" format
        let hash = hash_to_eth_signed_msg_hash(msg);

        // Undo adding the 27, to go back to the format expected below
        let mut sig_bytes: [u8; 65] = eth_sig.0;
        sig_bytes[64] -= 27;

        let sig = recoverable::Signature::from_bytes(&sig_bytes)
            .expect("Can't fail because size is known at compile time");

        let verifying_key = sig.recover_verifying_key_from_digest_bytes(&hash.0.into())?;
        Ok(verifying_key.into())
    }
}
