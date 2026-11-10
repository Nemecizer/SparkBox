package main

// The plain-profile peer uses upstream WireGuard, independently of the
// AmneziaWG client. A shared implementation cannot prove interoperability.
import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	at "github.com/amnezia-vpn/amneziawg-go/tun"
	wc "golang.zx2c4.com/wireguard/conn"
	wd "golang.zx2c4.com/wireguard/device"
	wt "golang.zx2c4.com/wireguard/tun"
	"sparkbox/tunnel/netstack"
)

type standardTun struct {
	at.Device
	events chan wt.Event
}

func (t *standardTun) Events() <-chan wt.Event { return t.events }

func standardPeer(c Config) (*netstack.Net, int, func(), error) {
	addrs, err := parseAddrs(c.Addresses)
	if err != nil {
		return nil, 0, nil, err
	}
	dev, network, err := netstack.CreateNetTUN(addrs, nil, 1420)
	if err != nil {
		return nil, 0, nil, err
	}
	adapter := &standardTun{Device: dev, events: make(chan wt.Event, 10)}
	go func() {
		defer close(adapter.events)
		for e := range dev.Events() {
			adapter.events <- wt.Event(e)
		}
	}()
	peer := wd.NewDevice(adapter, wc.NewDefaultBind(), wd.NewLogger(wd.LogLevelError, "test peer: "))
	cfg, err := uapi(c)
	if err == nil {
		err = peer.IpcSet(cfg)
	}
	if err == nil {
		err = peer.Up()
	}
	if err != nil {
		peer.Close()
		return nil, 0, nil, err
	}
	raw, err := peer.IpcGet()
	if err == nil {
		for _, line := range strings.Split(raw, "\n") {
			if v, ok := strings.CutPrefix(line, "listen_port="); ok {
				port, err := strconv.Atoi(v)
				if err == nil && port > 0 {
					return network, port, peer.Close, nil
				}
			}
		}
	}
	peer.Close()
	return nil, 0, nil, fmt.Errorf("upstream peer did not report its listening port")
}

// Exercises the actual JSON helper boundary, including the production-shaped
// /32 address, full-tunnel AllowedIPs and MTU. The optional path lets a release
// candidate be diagnosed without loading the owner's profile or Keychain.
func TestBundledHelperWithUpstreamWireGuard(t *testing.T) {
	serverPrivate, serverPublic := keypair(t)
	clientPrivate, clientPublic := keypair(t)
	network, port, closePeer, err := standardPeer(Config{PrivateKey: serverPrivate, Addresses: []string{"10.99.0.1/24"}, PeerPublicKey: clientPublic, AllowedIPs: []string{"10.99.0.2/32"}})
	if err != nil {
		t.Fatal(err)
	}
	defer closePeer()
	listener, err := network.ListenTCP(&net.TCPAddr{IP: net.ParseIP("10.99.0.1"), Port: 8080})
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	go http.Serve(listener, http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { io.WriteString(w, "upstream wireguard") }))
	binary := os.Getenv("SPARKBOX_TEST_HELPER")
	if binary == "" {
		binary = filepath.Join(t.TempDir(), "sparkbox-tunnel")
		if out, err := exec.Command("go", "build", "-o", binary, ".").CombinedOutput(); err != nil {
			t.Fatalf("build helper: %v: %s", err, out)
		}
	}
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	child := exec.CommandContext(ctx, binary)
	input, _ := child.StdinPipe()
	output, _ := child.StdoutPipe()
	if err := child.Start(); err != nil {
		t.Fatal(err)
	}
	defer func() { input.Close(); child.Wait() }()
	config := Config{PrivateKey: clientPrivate, Addresses: []string{"10.99.0.2/32"}, MTU: 1412, PeerPublicKey: serverPublic, Endpoint: fmt.Sprintf("127.0.0.1:%d", port), AllowedIPs: []string{"0.0.0.0/0"}, Keepalive: 25, SocksUser: "test", SocksPass: "temporary"}
	if err := json.NewEncoder(input).Encode(config); err != nil {
		t.Fatal(err)
	}
	lines := bufio.NewScanner(output)
	socks, connected := "", false
	for lines.Scan() {
		var e struct {
			Event           string
			Socks           string
			HandshakeAgeSec int64
		}
		if json.Unmarshal(lines.Bytes(), &e) != nil {
			continue
		}
		if e.Event == "ready" {
			socks = e.Socks
		}
		if e.Event == "stats" && e.HandshakeAgeSec >= 0 {
			connected = true
			break
		}
	}
	if !connected || socks == "" {
		t.Fatal("helper did not establish an upstream WireGuard handshake")
	}
	body, err := getViaSocks(t, socks, "test", "temporary", "http://10.99.0.1:8080/")
	if err != nil || body != "upstream wireguard" {
		t.Fatalf("tunneled HTTP failed: %q, %v", body, err)
	}
}
