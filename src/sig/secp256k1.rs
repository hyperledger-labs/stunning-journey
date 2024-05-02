//! Signer using the secp256k1 C-Library.

use super::hash_to_eth_signed_msg_hash;
use crate::abiencode::types::{Address, Hash, Signature};
use secp256k1::{
    self,
    ecdsa::{RecoverableSignature, RecoveryId},
    All, Message, PublicKey, Secp256k1, SecretKey,
};

pub use secp256k1::Error;

#[derive(Debug)]
pub struct Signer {
    secp: Secp256k1<All>,
    sk: SecretKey,
    addr: Address,
}

impl Signer {
    pub fn new<R: rand::Rng + rand::CryptoRng>(rng: &mut R) -> Self {
        let private_key_bytes: [u8; 32] = [
            0x24, 0x4F, 0xFC, 0x73, 0xC4, 0x48, 0xB5, 0x6D, 0xDB, 0xA6, 0xA7, 0xBF, 0xA8, 0xD5,
            0x8E, 0xD3, 0x60, 0x12, 0x61, 0x1D, 0xA8, 0x3D, 0x4C, 0xB8, 0x30, 0x25, 0xEA, 0x12,
            0xAC, 0xCF, 0x49, 0xFE,
        ];

        let secp = Secp256k1::new();

        let sk = SecretKey::from_slice(&private_key_bytes).expect("Invalid private key");

        let pk = PublicKey::from_secret_key(&secp, &sk);

        Self {
            secp,
            sk,
            addr: pk.into(),
        }
    }

    pub fn address(&self) -> Address {
        self.addr
    }

    /// Sign a hash using a Ethereum 65-byte recoverable signature.
    ///
    /// Note that this differs from transaction signatures, as it does not include
    /// the length. 64-byte recoverable signatures would be possible, but are not
    /// implemented here for simplicity.
    pub fn sign_eth(&self, msg: Hash) -> Signature {
        // Partially taken from https://github.com/synlestidae/ethereum-tx-sign/blob/master/src/lib.rs#L534

        // "\x19Ethereum Signed Message:\n32" format
        let hash = hash_to_eth_signed_msg_hash(msg);

        // We have to use sign_ecdsa_recoverable because the smart contract must be
        // able to recover the address. This gives us the additional information
        // needed for v.
        let sig = self
            .secp
            .sign_ecdsa_recoverable(&Message::from(hash), &self.sk);

        // Get the bytes from the signature.
        let (v, rs) = sig.serialize_compact();

        // [EIP-2](https://eips.ethereum.org/EIPS/eip-2) makes all signatures with a
        // non-canonical solution (s starts with the bit 1) invalid. From
        // openzeppelin ECDSA.sol: "EIP-2 still allows signature malleability for
        // ecrecover()", but openzeppelin intentionally prevents these solutions to
        // make signatures unique and not malleable. From testing the library does
        // already produce canonical signatures, this debug_assert is just to fail
        // early if that changes at some point.
        debug_assert!(rs[32] & 0x80 == 0);

        // According to [EIP-2098](https://eips.ethereum.org/EIPS/eip-2098), the
        // yParity (v) is offset by 27 so the value does not collide with other
        // binary prefixes used in Bitcoin. Ethereum just kept this offset.
        //
        // Ethereum and OpenZeppelin support compact signatures (see
        // [EIP-2098](https://eips.ethereum.org/EIPS/eip-2098)), which store "v" in
        // the first bit of s to bring the signature length from 65 bytes to 64
        // bytes. This implementation does not do that.
        //
        // Since [EIP-155](https://eips.ethereum.org/EIPS/eip-155) transaction
        // signatures additionally include the chain id by making v longer (abi
        // encoded it does not make a difference because v is stored in a 256-bit
        // slot). Openzeppelin does not do that and will not recover the address
        // from a signature that does, which is why we do not do this here.
        let v: u8 = 27 + v.to_i32() as u8;

        Signature::new(&rs, v)
    }

    /// Recover the Public Key from a signature.
    ///
    /// Hash is the hash of the data given to [Self::sign_eth()], it should not
    /// include the `Ethereum Signed Message` prefix.
    ///
    /// To get the Ethereum Address use `into()`.
    pub fn recover_signer(&self, hash: Hash, eth_sig: Signature) -> Result<Address, Error> {
        let hash = hash_to_eth_signed_msg_hash(hash);

        let rs = &eth_sig.0[..64];
        let v = eth_sig.0[64] - 27;

        let recid = RecoveryId::from_i32(v.into())?;
        let sig = RecoverableSignature::from_compact(rs, recid)?;

        let pk = self.secp.recover_ecdsa(&Message::from(hash), &sig)?;

        Ok(pk.into())
    }
}
