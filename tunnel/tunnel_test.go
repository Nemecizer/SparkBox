package main

// End-to-end tests: a second in-process device plays the Firewalla, serves
// HTTP inside the tunnel, and the client reaches it through SOCKS5, a port
// forward and a probe — for plain WireGuard and for AmneziaWG.

import (
	"bufio"
	"context"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"testing"
	"time"

	"golang.org/x/crypto/curve25519"
	"golang.org/x/net/proxy"
	"sparkbox/tunnel/netstack"
)

func keypair(t *testing.T) (priv, pub string) {
	t.Helper()
	k := make([]byte, 32)
	if _, err := rand.Read(k); err != nil {
		t.Fatal(err)
	}
	k[0] &= 248
	k[31] = (k[31] & 127) | 64
	p, err := curve25519.X25519(k, curve25519.Basepoint)
	if err != nil {
		t.Fatal(err)
	}
	return base64.StdEncoding.EncodeToString(k), base64.StdEncoding.EncodeToString(p)
}

var awgSettings = map[string]string{
	"jc": "4", "jmin": "40", "jmax": "70", "s1": "15", "s2": "21",
	"h1": "1294727403", "h2": "2093812374", "h3": "981244313", "h4": "1748293015",
	"i1": "<b 0xc0ffee><r 24><t>",
}

// The fake Firewalla runs in its own process: amneziawg-go keeps the AmneziaWG
// header values in package globals, so two devices in one process would always
// agree and a mismatch could never be detected.
const peerEnv = "SPARKBOX_TEST_PEER"

func TestMain(m *testing.M) {
	if raw := os.Getenv(peerEnv); raw != "" {
		runTestPeer(raw)
		return
	}
	os.Exit(m.Run())
}

// runTestPeer serves HTTP at 10.99.0.1:8080 inside the tunnel until stdin closes.
func runTestPeer(raw string) {
	var c Config
	if err := json.Unmarshal([]byte(raw), &c); err != nil {
		panic(err)
	}
	var network *netstack.Net
	var port int
	if len(c.Obfuscation) == 0 {
		n, p, closePeer, err := standardPeer(c)
		if err != nil {
			panic(err)
		}
		defer closePeer()
		network, port = n, p
	} else {
		srv, err := Start(c, func(string) {})
		if err != nil {
			panic(err)
		}
		defer srv.Close()
		network, port = srv.ServerNet(), srv.Stats().ListenPort
	}
	l, err := network.ListenTCP(&net.TCPAddr{IP: net.ParseIP("10.99.0.1"), Port: 8080})
	if err != nil {
		panic(err)
	}
	go http.Serve(l, http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		io.WriteString(w, "hello from the spark network")
	}))
	fmt.Printf("port=%d\n", port)
	io.Copy(io.Discard, os.Stdin)
}

// pair starts the fake Firewalla (separate process) and a client tunnel to it.
func pair(t *testing.T, serverObf, clientObf map[string]string) (client *Tunnel) {
	return pairKeepalive(t, serverObf, clientObf, 25)
}

func pairKeepalive(t *testing.T, serverObf, clientObf map[string]string, keepalive int) *Tunnel {
	t.Helper()
	sPriv, sPub := keypair(t)
	cPriv, cPub := keypair(t)

	peerCfg, _ := json.Marshal(Config{
		PrivateKey: sPriv, Addresses: []string{"10.99.0.1/24"},
		PeerPublicKey: cPub, AllowedIPs: []string{"10.99.0.2/32"},
		Obfuscation: serverObf,
	})
	cmd := exec.Command(os.Args[0], "-test.run=^$")
	cmd.Env = append(os.Environ(), peerEnv+"="+string(peerCfg))
	stdin, _ := cmd.StdinPipe()
	stdout, _ := cmd.StdoutPipe()
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { stdin.Close(); cmd.Wait() })
	var port int
	if _, err := fmt.Fscanf(bufio.NewReader(stdout), "port=%d\n", &port); err != nil || port == 0 {
		t.Fatalf("fake Firewalla didn't start: %v", err)
	}

	client, err := Start(Config{
		PrivateKey: cPriv, Addresses: []string{"10.99.0.2/32"},
		PeerPublicKey: sPub, Endpoint: net.JoinHostPort("127.0.0.1", strconv.Itoa(port)),
		AllowedIPs: []string{"10.99.0.0/24"}, Keepalive: keepalive,
		Obfuscation: clientObf, SocksUser: "sparkbox", SocksPass: "s3cret",
	}, func(string) {})
	if err != nil {
		t.Fatalf("client: %v", err)
	}
	t.Cleanup(client.Close)
	return client
}

func getViaSocks(t *testing.T, socks, user, pass, url string) (string, error) {
	t.Helper()
	d, err := proxy.SOCKS5("tcp", socks, &proxy.Auth{User: user, Password: pass}, proxy.Direct)
	if err != nil {
		return "", err
	}
	tr := &http.Transport{DialContext: func(ctx context.Context, n, a string) (net.Conn, error) {
		return d.(proxy.ContextDialer).DialContext(ctx, n, a)
	}}
	c := &http.Client{Transport: tr, Timeout: 10 * time.Second}
	resp, err := c.Get(url)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	b, err := io.ReadAll(resp.Body)
	return string(b), err
}

func exercise(t *testing.T, c *Tunnel) {
	t.Helper()
	body, err := getViaSocks(t, c.SocksAddr(), "sparkbox", "s3cret", "http://10.99.0.1:8080/")
	if err != nil || body != "hello from the spark network" {
		t.Fatalf("via SOCKS: %q, %v", body, err)
	}

	bound, err := c.Forward("127.0.0.1:0", "10.99.0.1:8080")
	if err != nil {
		t.Fatal(err)
	}
	resp, err := http.Get("http://" + bound + "/")
	if err != nil {
		t.Fatalf("via forward: %v", err)
	}
	b, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	if string(b) != "hello from the spark network" {
		t.Fatalf("via forward: %q", b)
	}

	if _, err := c.Probe("10.99.0.1:8080", 3*time.Second); err != nil {
		t.Fatalf("probe: %v", err)
	}
	s := c.Stats()
	if s.HandshakeAgeSec < 0 || s.RxBytes == 0 || s.TxBytes == 0 {
		t.Fatalf("stats after traffic: %+v", s)
	}
}

func TestPlainWireGuard(t *testing.T) {
	exercise(t, pair(t, nil, nil))
}

func TestAmneziaWG(t *testing.T) {
	exercise(t, pair(t, awgSettings, awgSettings))
}

func TestAmneziaWGMismatchGetsNoHandshake(t *testing.T) {
	wrong := map[string]string{}
	for k, v := range awgSettings {
		wrong[k] = v
	}
	wrong["h1"] = "12345"
	c := pair(t, awgSettings, wrong)
	if _, err := c.Probe("10.99.0.1:8080", 2*time.Second); err == nil {
		t.Fatal("probe succeeded although the obfuscation settings differ")
	}
	if s := c.Stats(); s.HandshakeAgeSec != -1 {
		t.Fatalf("unexpected handshake: %+v", s)
	}
}

func TestSocksRejectsWrongPassword(t *testing.T) {
	c := pair(t, nil, nil)
	if _, err := getViaSocks(t, c.SocksAddr(), "sparkbox", "wrong", "http://10.99.0.1:8080/"); err == nil {
		t.Fatal("SOCKS accepted a wrong password")
	}
}

func TestUnknownSettingIsRejected(t *testing.T) {
	priv, pub := keypair(t)
	_, err := uapi(Config{PrivateKey: priv, PeerPublicKey: pub, Obfuscation: map[string]string{"bogus": "1"}})
	if err == nil || !strings.Contains(err.Error(), "bogus") {
		t.Fatalf("got %v", err)
	}
}

func TestConnectWithoutKeepaliveOrApplicationTraffic(t *testing.T) {
	c := pairKeepalive(t, nil, nil, 0)
	deadline := time.Now().Add(5 * time.Second)
	for c.Stats().HandshakeAgeSec < 0 && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	s := c.Stats()
	if s.HandshakeAgeSec < 0 || s.UDPSent == 0 || s.UDPReceived == 0 {
		t.Fatalf("no initial handshake: %+v", s)
	}
	exercise(t, c)
}
