use super::*;

/*
    struct StringContainer {
        string s;
    }
    function String_0() public pure returns(bytes memory) {
        StringContainer memory d;
        d.s = "";
        return abi.encode(d);
    }
    function String_1() public pure returns(bytes memory) {
        StringContainer memory d;
        d.s = "0";
        return abi.encode(d);
    }
    function String_31() public pure returns(bytes memory) {
        StringContainer memory d;
        d.s = "0123456789abcdef0123456789abcde";
        return abi.encode(d);
    }
    function String_32() public pure returns(bytes memory) {
        StringContainer memory d;
        d.s = "0123456789abcdef0123456789abcdef";
        return abi.encode(d);
    }
    function String_33() public pure returns(bytes memory) {
        StringContainer memory d;
        d.s = "0123456789abcdef0123456789abcdef0";
        return abi.encode(d);
    }
    function String_64() public pure returns(bytes memory) {
        StringContainer memory d;
        d.s = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        return abi.encode(d);
    }
*/

macro_rules! make_size_test {
    ($name:ident, $input:literal, $expected:literal) => {
        #[test]
        fn $name() {
            #[derive(Serialize, Debug)]
            struct StringContainer {
                s: &'static str,
            }

            let d = StringContainer { s: $input };

            serialize_and_compare(&d, $expected)
        }
    };
}

make_size_test!(
    length_0,
    "",
    "
0000000000000000000000000000000000000000000000000000000000000020 // d offset
    0000000000000000000000000000000000000000000000000000000000000020 // d.s offset
        0000000000000000000000000000000000000000000000000000000000000000 // d.s length
"
);

make_size_test!(
    length_1,
    "0",
    "
0000000000000000000000000000000000000000000000000000000000000020 // d offset
    0000000000000000000000000000000000000000000000000000000000000020 // d.s offset
        0000000000000000000000000000000000000000000000000000000000000001 // d.s length
        3000000000000000000000000000000000000000000000000000000000000000 // d.s
"
);

make_size_test!(
    length_31,
    "0123456789abcdef0123456789abcde",
    "
0000000000000000000000000000000000000000000000000000000000000020 // d offset
    0000000000000000000000000000000000000000000000000000000000000020 // d.s offset
        000000000000000000000000000000000000000000000000000000000000001f // d.s length
        3031323334353637383961626364656630313233343536373839616263646500 // d.s
"
);

make_size_test!(
    length_32,
    "0123456789abcdef0123456789abcdef",
    "
0000000000000000000000000000000000000000000000000000000000000020 // d offset
    0000000000000000000000000000000000000000000000000000000000000020 // d.s offset
        0000000000000000000000000000000000000000000000000000000000000020 // d.s length
        3031323334353637383961626364656630313233343536373839616263646566 // d.s
"
);

make_size_test!(
    length_33,
    "0123456789abcdef0123456789abcdef0",
    "
0000000000000000000000000000000000000000000000000000000000000020 // d offset
    0000000000000000000000000000000000000000000000000000000000000020 // d.s offset
        0000000000000000000000000000000000000000000000000000000000000021 // d.s length
        3031323334353637383961626364656630313233343536373839616263646566 // d.s
        3000000000000000000000000000000000000000000000000000000000000000 // d.s
"
);

make_size_test!(
    length_64,
    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    "
0000000000000000000000000000000000000000000000000000000000000020 // d offset
    0000000000000000000000000000000000000000000000000000000000000020 // d.s offset
        0000000000000000000000000000000000000000000000000000000000000040 // d.s length
        3031323334353637383961626364656630313233343536373839616263646566 // d.s
        3031323334353637383961626364656630313233343536373839616263646566 // d.s
"
);
