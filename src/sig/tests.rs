use crate::{
    abiencode::{self, as_bytes},
    Hash,
};
use rand::{rngs::StdRng, SeedableRng};
use serde::Serialize;
use uint::hex::ToHex;

fn data() -> Hash {
    /*
    ```solidity
    function verify_sig(address signer, bytes memory sig) public pure {
        bytes memory d;
        d = "\xa1\xa2\xa3\xa4";
        require(Sig.verify(abi.encode(d), sig, signer), "invalid signature");
    }
    ```
    */

    #[derive(Serialize, Debug)]
    #[serde(transparent)]
    struct Bytes {
        #[serde(with = "as_bytes")]
        value: [u8; 4],
    }
    let d = Bytes {
        value: [0xa1, 0xa2, 0xa3, 0xa4],
    };

    abiencode::to_hash(&d).unwrap()
}

macro_rules! make_compare_hardcoded {
    ($name:ident, $signer:ty, $address:literal, $expected_sig:literal) => {
        #[test]
        fn $name() {
            // This test may break in the future (e.g. if the dependency changes
            // internally), they exist to allow checking if a signature is valid
            // on-chain without needing a real blockchain or ganache.

            // Do not use that on any real device, this is just for testing.
            let mut rng = StdRng::seed_from_u64(0);
            let signer = <$signer>::new(&mut rng);
            let sig = signer.sign_eth(data());

            println!("Address: {}", signer.address().0.encode_hex::<String>());
            println!("Sig: 0x{}", sig.0.encode_hex::<String>());

            let address = $address;
            let expected_sig = $expected_sig;

            // Sanity checks for macro user input
            assert_eq!(address.len(), 20 * 2);
            assert_eq!(expected_sig.len(), 2 + 65 * 2);

            assert_eq!(signer.address().0.encode_hex::<String>(), address);
            assert_eq!(sig.0.encode_hex::<String>(), &expected_sig[2..]);
        }
    };
}

macro_rules! make_a_to_b {
    ($name:ident, $signer:ty, $verifier:ty) => {
        #[test]
        fn $name() {
            // Do not use that on any real device, this is just for testing.
            let mut rng = StdRng::seed_from_u64(0);
            let signer = <$signer>::new(&mut rng);
            let msg = data();
            let sig = signer.sign_eth(msg);

            println!("Address: {}", signer.address().0.encode_hex::<String>());
            println!("Sig: 0x{}", sig.0.encode_hex::<String>());

            let verifier = <$verifier>::new(&mut rng);
            let address = verifier.recover_signer(msg, sig).unwrap();

            assert_eq!(address, signer.address());
        }
    };
}

// Note that the outputs of the following do not necessary have to be equal,
// they may depend on how exactly each library uses the random number generator.
#[cfg(feature = "secp256k1")]
make_compare_hardcoded!(
    secp256k1_sign,
    super::secp256k1::Signer,
    "7b7e212652b9c3755c4e1f1718a142dde3817523",
    "0x2cdd209aec67c42d8925c6f2c92a9fb5fb890e975c812b9ab0c1f0edb38168ef0d46e9e0481aff663c1a04c9354d77ef51228cd18bf7d2cf255ce651c51a55b41c"
);

#[cfg(feature = "k256")]
make_compare_hardcoded!(
    k256_sign,
    super::k256::Signer,
    "7b7e212652b9c3755c4e1f1718a142dde3817523",
    "0x2cdd209aec67c42d8925c6f2c92a9fb5fb890e975c812b9ab0c1f0edb38168ef0d46e9e0481aff663c1a04c9354d77ef51228cd18bf7d2cf255ce651c51a55b41c"
);

#[cfg(feature = "secp256k1")]
make_a_to_b!(
    secp256k1_to_secp256k1,
    super::secp256k1::Signer,
    super::secp256k1::Signer
);

#[cfg(feature = "k256")]
make_a_to_b!(k256_to_k256, super::k256::Signer, super::k256::Signer);

#[cfg(all(feature = "secp256k1", feature = "k256"))]
make_a_to_b!(
    secp256k1_to_k256,
    super::secp256k1::Signer,
    super::k256::Signer
);

#[cfg(all(feature = "secp256k1", feature = "k256"))]
make_a_to_b!(
    k256_to_secp256k1,
    super::k256::Signer,
    super::secp256k1::Signer
);
