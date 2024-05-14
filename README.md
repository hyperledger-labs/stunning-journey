# Perun Blockchain-Agnostic State Channels in Rust
Rust-perun allows using Perun channels (currently only 2-party payment channels)
on embedded devices, which is difficult when using Go. Since embedded devices
usually don't have enough computing power to watch the Ethereum blockchain the
Rust-perun repo uses an external service (implemented using Go-perun) for
Watching the blockchain for disputes and for funding a new channel.

## Getting started
```bash
# Initialize submodules
git submodule init
git submodule update

# (Optional) Install tools (if not done already).
sudo apt-get install libudev-dev
cargo install cargo-flash

# Execute all tests
cargo test --all-features

# Run go-integration example/walkthrough (run in separate terminals)
# Ganache is optional, if it isn't running we're using the SimulatedBackend.
ganache -e 100000000000000 -s 1024 -b 5
cd examples/go-integration; go run . ; cd -
cargo run --example go-integration

# Run old Example/Walkthrough (can be configured at the top with constants)
cargo run --example lowlevel_basic_channel

# Add the microcontroller target and install dependancies.
rustup +nightly target install thumbv7em-none-eabihf
rustup update
sudo apt-get install gcc-arm-none-eabi

# Compile without std (the example above requires std)
cargo +nighly build --target thumbv7em-none-eabi --no-default-features -F k256

# (Optional) Install qemu (if not done already).
sudo apt-get install qemu-system-arm

# Compile example without std and run in qemu
# - does not have communication with Go
# - currently requires the nightly compiler (due to the chosen allocator)
# - `--release` is needed to reduce the binary size so it fits into FLASH
cargo +nightly run --example go-integration --target thumbv7m-none-eabi --no-default-features -F nostd-example --release
```

### Run (new) demo on real hardware
Configure the Laptop/Compuers Ethernet adapter to the static IP 10.0.0.1/24,
which is the hard-coded IP expected by the device.

For the communication to work the Jumpers JP6 and JP7 must be set (connected),
along (most likely) some others. See the board documentation if the application
runs on the device but cannot communicate.

```bash
# Initialize submodules
git submodule init
git submodule update

# The demo requires nightly, so either set that as a default or use +nightly in each cargo command.
rustup override set nightly-x86_64-unknown-linux-gnu

# (Optional) Install tools (if not done already).
cargo install cargo-flash

# (Optional) Run Ganache in Terminal 1
ganache -e 100000000000000 -s 1024 -b 5

# Run Go-side in Terminal 2
cd examples/go-integration; go run . ; cd -

# Compile and Flash to connected device
cargo +nightly flash --chip STM32F439ZITx --target thumbv7em-none-eabihf -p cortex-m-demo --release
```

#### Debugging
In addition to ganache and the go-side:
```bash
# Compile with debug information (I couldn't get custom profiles to work with cargo-flash)
# Release optimizations are required to run (otherwise it needs too much memory)
# Using this profile additionally adds debug information, though it is not perfect.
cargo +nightly build --target thumbv7em-none-eabihf -p cortex-m-demo --profile=release-with-debug

# Start openocd in Terminal 3
cd cortex-m-demo; openocd ; cd -

# Start gdb in Terminal 4 (alternative binary name: gdb-multiarch)
arm-none-eabi-gdb -q target/thumbv7em-none-eabihf/release-with-debug/cortex-m-demo
```

In gdb enter the following:
```bash
# Connect to openocd
target extended-remote :3333

# Set breakpoints for panics
break DefaultHandler
break HardFault
break rust_begin_unwind

# (Optional) enable semihosting if needed (at the moment it isn't, but it can be useful when using panic_semihosting)
monitor arm semihosting enable

# Flash compiled binary to device (don't forget to recompile and load after making changes)
load

# Use the normal gdb commands to set breakpoints or run the application

# To restart the application without flashing:
run
```

## cortex-m-demo I/O
- Green LED: Toggles after every (debounced) button press to indicate it was
  registered
- Blue LED: Toggles every Second to indicate that the application has not
  crashed
- Red LED: Toggles if the button press was invalid (for example because the
  channel is already closed). The demo will continue to work normally if the red
  LED roggles.
- USER Button (Blue, B1): Send 100 WEI to the other participant
- PA0 (D32), located in CN10, marked as "TIMER", the 3rd pin from the bottom
  (side with the buttons) on the inner side: Send a normal channel closure
  (is_final=true) by connecting it to any GND pin. Only valid if the channel is
  Active.
- PE0 (D34), located in CN10, marked as "TIMER", the 1st pin from the bottom
  (side with the buttons) on the inner side: Send a force close (dispute
  request) by connecting it to any GND pin. Only valid if the channel is Active.
- PE2 (D31), located in CN10, marked as "QSPI", the 5th pin from the bottom
  (side with the buttons) on the inner side: Propose a channel by connecting it
  to any GND pin. Only valid if in the `Configured` (Idle) state (we have no
  active channel and are not already in the process of proposing one).

## go-side I/O
The go-side can be controlled via TCP port 2222, allowing you to send channel
proposals, updates and be able to close the channel. Additionally it provides a
`status` command that prints the state of all channels known to the go-side. As
there is only one active channel on the rust-side. Be careful to use the
commands with an index (in the list shown with `status`, not the channel ID)
when there are channels to other participants or when communicating with
multiple embedded devices, otherwise you may close a different channel.

The easiest way to use this is with netcat:
```bash
ncat 127.0.0.1 2222
```

## Feature Flags
- `std` (default)
- `k256` (default) Use [`k256`](https://crates.io/crates/k256) for signatures
- `secp256k1` Use [`secp256k1`](https://crates.io/crates/secp256k1) for signatures (implies `std`)

## Limitations

<table>
  <th>
    <td>rust-perun</td>
    <td><a href="https://github.com/hyperledger-labs/go-perun">go-perun</a></td>
    <td><a href="https://github.com/hyperledger-labs/perun-eth-contracts/">perun-eth-contracts</a></td>
  </th>
  <tr>
    <td>API</td>
    <td>
        <details><summary>low-level</summary>
            The application stores a reference to all channels and forwards incoming messages and responses.
        </details>
    </td>
    <td>
        <details><summary>high-level</summary>
            The sdk stores channel objects and handles communication implicitly, providing only a few functions to the application.
        </details>
    </td>
    <td>N/A</td>
  </tr>
  <tr>
    <td>Can run on embedded devices</td>
    <td>Yes</td>
    <td>No</td>
    <td>N/A</td>
  </tr>
  <tr>
    <td>Remote Funding/Watching possible+required</td>
    <td>Yes</td>
    <td>No</td>
    <td>N/A</td>
  </tr>
  <tr>
    <td>Works with Non-Ethereum blockchains</td>
    <td>No</td>
    <td>Yes</td>
    <td>N/A</td>
  </tr>
  <tr>
    <td>2+ Assets</td>
    <td>
        <details><summary>No</summary>
            Limited by the data structures used (fixed-size arrays with const generics). Should be relatively easy to fix by using heapless data structures on no_std or normal vectors on std.
        </details>
    </td>
    <td>Yes</td>
    <td>Yes</td>
  </tr>
  <tr>
    <td>3+ Participants</td>
    <td>
        <details><summary>No</summary>
            Limited by the data structures used (fixed-size arrays with const generics). Should be relatively easy to fix by using heapless data structures on no_std or normal vectors on std.
        </details>
    </td>
    <td>
        <details><summary>No</summary>
            Limited by the network communication code.
        </details>
    </td>
    <td>Yes</td>
  </tr>
  <tr>
    <td>Communication on embedded device</td>
    <td>
        <details><summary>Yes</summary>
            <ul>
                <li>Incomming connections: Yes</li>
                <li>Embedded device to embedded device: Possible but not tested. The cortex-m-demo contains all code to do this, but it currently uses hard-coded wire addresses.</li>
                <li>Embedded device to watcher/funder: Yes</li>
                <li>Embedded device to normal go-perun client: Yes</li>
                <li>Normal go-perun client to embedded device: Yes</li>
            </ul>
        </details>
    </td>
    <td>No</td>
    <td>N/A</td>
  </tr>
  <tr>
    <td>Implicit/Automatic handling of incoming messages</td>
    <td>
        <details><summary>No</summary>
            At the moment, given the low-level interface, the application is responsible for storing channel objects and giving incomming messages to the rust-perun code. We chose to do it this way to avoid requireing an async runtime or blocking the execution until receiving something. At some point this will likely be changed to one of the following, providing a higher-level interface that is easier to use: Additional async functions that handle reply messages; polling system that automatically tries to receive incomming messages, updating the channel state automatically. Both are similar to how go-perun currently does it, though with some adjustments/changes needed due to using Rust and running on bare metal in no_std.
        </details>
    </td>
    <td>Yes</td>
    <td>N/A</td>
  </tr>
  <tr>
    <td>Can have multiple channels</td>
    <td>
      Library: Yes<br>
      Demo: No<br>
      <details><summary>Hardware: Until we run out of memory</summary>
        The trivial solution of one long-living TCP connection per channel likely won't work due to memory constraints. It is possible to use a single connection for multiple channels (e.g. having another server relaying the messages) or to only use short-lived connections (which won't work during channel proposals/setup). The application can also use a different protocol for communication, but that will require the other participants to use that, too, or a relaying server. Neither of those is implemented in the demo, all of those are possible with the library.
      </details>
    </td>
    <td>Yes</td>
    <td>N/A</td>
  </tr>
</table>

## Copyright

Copyright 2023 - See [NOTICE file](NOTICE) for copyright holders.
Use of the source code is governed by the Apache 2.0 license that can be found in the [LICENSE file](LICENSE).

Contact us at [info@perun.network](mailto:info@perun.network).
