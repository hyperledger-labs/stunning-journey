package main

import (
	"context"
	"crypto/rand"
	"fmt"
	"go-integration/control"
	remote "go-integration/perun-remote"
	"math/big"
	"net"
	"os"
	"os/signal"
	"strings"
	"syscall"
	"time"

	"github.com/ethereum/go-ethereum/accounts"
	"github.com/ethereum/go-ethereum/accounts/abi/bind/backends"
	"github.com/ethereum/go-ethereum/core"
	"github.com/ethereum/go-ethereum/ethclient"
	"github.com/ethereum/go-ethereum/params"
	ethchannel "github.com/perun-network/perun-eth-backend/channel"
	ethwallet "github.com/perun-network/perun-eth-backend/wallet"
	phd "github.com/perun-network/perun-eth-backend/wallet/hd"
	"github.com/sirupsen/logrus"
	"perun.network/go-perun/apps/payment"
	"perun.network/go-perun/channel"
	"perun.network/go-perun/client"
	perunlogrus "perun.network/go-perun/log/logrus"
	"perun.network/go-perun/wallet"
	"perun.network/go-perun/watcher/local"
	wirenet "perun.network/go-perun/wire/net"
	"perun.network/go-perun/wire/net/simple"
	"perun.network/go-perun/wire/protobuf"
)

func ToWei(value int64, denomination string) *big.Int {
	// if denomination == "ether" or denomination == "eth":
	var m int64
	switch strings.ToLower(denomination) {
	case "ether":
		m = params.Ether
	case "eth":
		m = params.Ether
	case "gwei":
		m = params.GWei
	case "wei":
		m = params.Wei
	default:
		panic("Unknown denomination")
	}
	return new(big.Int).Mul(big.NewInt(value), big.NewInt(m))
}

func setup_blockchain(accounts ...accounts.Account) (ethchannel.ContractInterface, *big.Int) {
	contract_interface, chain_id, err := setup_ganache(accounts...)
	if err != nil {
		fmt.Printf("Using SimulatedBackend (fallback) because we could not connect to ganache: %v\n", err)
		return setup_simbackend(accounts...)
	}
	fmt.Println("Using Ganache")
	return contract_interface, chain_id
}

func setup_simbackend(accounts ...accounts.Account) (ethchannel.ContractInterface, *big.Int) {
	genesis_alloc := make(core.GenesisAlloc, len(accounts))
	for _, acc := range accounts {
		genesis_alloc[acc.Address] = core.GenesisAccount{Balance: ToWei(1_000_000, "ether")}
	}
	sb := backends.NewSimulatedBackend(
		genesis_alloc,
		30_000_000,
	)
	go func() {
		for {
			sb.Commit()
			time.Sleep(2000 * time.Millisecond)
		}
	}()
	chain_id := sb.Blockchain().Config().ChainID
	return sb, chain_id
}

func setup_ganache(accounts ...accounts.Account) (ethchannel.ContractInterface, *big.Int, error) {
	contract_interface, err := ethclient.Dial("ws://127.0.0.1:8545")
	if err != nil {
		return nil, nil, fmt.Errorf("Could not dial: %w", err)
	}
	chain_id, err := contract_interface.ChainID(context.Background())
	if err != nil {
		return nil, nil, fmt.Errorf("Could not get chainID: %w", err)
	}
	return contract_interface, chain_id, nil
}

func main() {
	perunlogrus.Set(logrus.TraceLevel, &logrus.TextFormatter{})

	w := NewSimpleWallet()

	// Wallet/Accounts
	// Command to run ganache:
	// `ganache-cli -e 100000000000000 -b 5 -s 1024`
	not_so_private_keys := []string{
		"0xf59fcb369b2caf390bf8398b18e4172ce85ef01111903b603f3b3e1f33e80050",
		"0x4bcebba3fc0cc4fdc2bfb6c10ac2cbf85367a75f2921a75bb76b9440616c87e4",
		"0xec951c901d6b68a8e3b0faf34ef93d0e03d219efd6a0d996ebaf140632a465fd",
	}
	adjudicator_account := w.ImportFromSecretKeyHex(not_so_private_keys[0][2:])
	deployer_account := w.ImportFromSecretKeyHex(not_so_private_keys[1][2:])
	funder_account := w.ImportFromSecretKeyHex(not_so_private_keys[2][2:])

	contract_interface, chain_id := setup_blockchain(adjudicator_account, deployer_account, funder_account)

	cb := ethchannel.NewContractBackend(
		contract_interface,
		ethchannel.MakeChainID(chain_id),
		NewChainIdAwareTransactor(w, chain_id),
		1,
	)

	channel.RegisterDefaultApp(&payment.Resolver{})

	// Deploy contracts
	adjAddr, err := ethchannel.DeployAdjudicator(context.Background(), cb, deployer_account)
	if err != nil {
		panic(err)
	}
	eth_holder, err := ethchannel.DeployETHAssetholder(context.Background(), cb, adjAddr, deployer_account)
	if err != nil {
		panic(err)
	}

	// Setup dependency injection objects
	funder := ethchannel.NewFunder(cb)
	funder.RegisterAsset(
		ethchannel.Asset{
			ChainID: ethchannel.ChainID{
				Int: chain_id,
			},
			AssetHolder: ethwallet.Address(eth_holder),
		},
		ethchannel.NewETHDepositor(),
		funder_account,
	)
	adjudicator := ethchannel.NewAdjudicator(
		cb,
		adjAddr,
		funder_account.Address,
		adjudicator_account,
	)
	perunID := simple.NewAddress("Alice")
	dialer := simple.NewTCPDialer(time.Minute)
	dialer.Register(simple.NewAddress("Bob"), "192.168.0.126:1234")
	bus := wirenet.NewBus(
		simple.NewAccount(perunID),
		dialer,
		protobuf.Serializer(),
	)
	wallet, err := phd.NewWallet(w, accounts.DefaultBaseDerivationPath.String(), 0)
	if err != nil {
		panic(err)
	}
	watcher_for_client, err := local.NewWatcher(adjudicator)
	if err != nil {
		panic(err)
	}
	c, err := client.New(perunID, bus, funder, adjudicator, wallet, watcher_for_client)
	if err != nil {
		panic(err)
	}
	bob_account, err := wallet.NewAccount()
	if err != nil {
		panic(err)
	}

	controlService := control.NewControlService(c, eth_holder, funder_account.Address)

	var proposalHandler client.ProposalHandler = ProposalHandler{
		addr:           bob_account.Address(),
		controlService: &controlService,
	}
	var updateHandler client.UpdateHandler = UpdateHandler{}

	listener, err := simple.NewTCPListener(":1337")
	if err != nil {
		panic(err)
	}

	go c.Handle(proposalHandler, updateHandler)
	go bus.Listen(listener)

	watcher_for_service, err := local.NewWatcher(adjudicator)
	if err != nil {
		panic(err)
	}
	server, err := remote.NewServer(
		remote.NewWatcherService(watcher_for_service, adjudicator),
		remote.NewFunderService(funder), 1338)
	if err != nil {
		panic(err)
	}
	go server.Serve()
	defer server.Close()

	// Listener for giving the EthHolder address to Rust (only needed for example)
	go func() {
		// Listen for any connection attempt on port 1338 and send out some
		// information like the ETH holder address. (needed for this example,
		// we're assuming the application already knows these values (for now
		// at least))
		l, err := net.Listen("tcp", ":1339")
		if err != nil {
			panic(err)
		}
		for {
			conn, err := l.Accept()
			if err != nil {
				panic(err)
			}

			// Note that using two `conn.Write` calls here is not ideal, as it
			// causes two separate 20-byte payload TCP segments.
			_, err = conn.Write(eth_holder.Bytes())
			if err != nil {
				panic(err)
			}
			_, err = conn.Write(funder_account.Address.Bytes())
			if err != nil {
				panic(err)
			}

			conn.Close()
		}
	}()

	// Control server
	go func() {
		err := controlService.Run()
		if err != nil {
			panic(err)
		}
	}()

	// Wait for Ctrl+C
	println("Press Ctrl+C to stop")
	stop := make(chan os.Signal, 1)
	signal.Notify(stop, syscall.SIGINT, syscall.SIGTERM)
	<-stop

	c.Close()
	bus.Close()
	println("Done")
}

type ProposalHandler struct {
	addr           wallet.Address
	controlService *control.ControlService
}

// HandleProposal implements client.ProposalHandler
func (ph ProposalHandler) HandleProposal(proposal client.ChannelProposal, res *client.ProposalResponder) {
	println("HandleProposal(): ", proposal, res)

	var nonce_share [32]byte
	_, err := rand.Read(nonce_share[:])
	if err != nil {
		panic(err)
	}

	ch, err := res.Accept(context.Background(), &client.LedgerChannelProposalAccMsg{
		BaseChannelProposalAcc: client.BaseChannelProposalAcc{
			ProposalID: proposal.Base().ProposalID,
			NonceShare: nonce_share,
		},
		Participant: ph.addr,
	})
	ph.controlService.RegisterChannel(ch)
	if err != nil {
		panic(err)
	}
}

type UpdateHandler struct{}

// HandleUpdate implements client.UpdateHandler
func (UpdateHandler) HandleUpdate(state *channel.State, update client.ChannelUpdate, res *client.UpdateResponder) {
	println("HandleUpdate(): ", state, res)

	err := res.Accept(context.Background())
	if err != nil {
		panic(err)
	}
}
