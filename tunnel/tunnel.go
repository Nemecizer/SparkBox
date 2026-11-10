// Package main is sparkbox-tunnel, SparkBox's user-space WireGuard / AmneziaWG
// client. It never creates a network interface or changes routes: traffic
// enters the tunnel only through the SOCKS5 proxy and port forwards it opens on
// 127.0.0.1, so nothing else on the Mac is affected.
package main

import (
	"context"
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"io"
	"net"
	"net/netip"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/amnezia-vpn/amneziawg-go/conn"
	"github.com/amnezia-vpn/amneziawg-go/device"
	"sparkbox/tunnel/netstack"
)

// Config is everything needed to bring the tunnel up. SparkBox builds it from
// the imported .conf file (secrets come from the Keychain).
type Config struct {
	PrivateKey    string   `json:"privateKey"` // base64, as in the .conf
	Addresses     []string `json:"addresses"`  // tunnel addresses, CIDR or bare
	DNS           []string `json:"dns"`
	MTU           int      `json:"mtu"`
	PeerPublicKey string   `json:"peerPublicKey"`
	PresharedKey  string   `json:"presharedKey"`
	Endpoint      string   `json:"endpoint"` // ip:port (SparkBox resolves names first)
	AllowedIPs    []string `json:"allowedIps"`
	Keepalive     int      `json:"keepalive"`
	// AmneziaWG settings, lower-case keys as in the .conf (jc, jmin, jmax, s1, s2,
	// h1-h4, i1-i5, j1-j3, itime). Empty means plain WireGuard.
	Obfuscation map[string]string `json:"obfuscation"`
	SocksUser   string            `json:"socksUser"`
	SocksPass   string            `json:"socksPass"`
	// ListenPort is only set by tests that act as the server side.
	ListenPort int `json:"listenPort,omitempty"`
}

// Tunnel is one running WireGuard device plus its local entry points.
type Tunnel struct {
	dev      *device.Device
	activity *activityBind
	tnet     *netstack.Net
	socks    net.Listener

	mu       sync.Mutex
	forwards map[string]net.Listener
}

var awgKeys = map[string]bool{
	"jc": true, "jmin": true, "jmax": true, "s1": true, "s2": true,
	"h1": true, "h2": true, "h3": true, "h4": true,
	"i1": true, "i2": true, "i3": true, "i4": true, "i5": true,
	"j1": true, "j2": true, "j3": true, "itime": true,
}

func keyHex(b64 string) (string, error) {
	raw, err := base64.StdEncoding.DecodeString(strings.TrimSpace(b64))
	if err != nil || len(raw) != 32 {
		return "", fmt.Errorf("not a WireGuard key")
	}
	return hex.EncodeToString(raw), nil
}

// uapi renders the config in wireguard-go's IPC format.
func uapi(c Config) (string, error) {
	var b strings.Builder
	priv, err := keyHex(c.PrivateKey)
	if err != nil {
		return "", fmt.Errorf("private key: %w", err)
	}
	fmt.Fprintf(&b, "private_key=%s\n", priv)
	if c.ListenPort > 0 {
		fmt.Fprintf(&b, "listen_port=%d\n", c.ListenPort)
	}
	keys := make([]string, 0, len(c.Obfuscation))
	for k := range c.Obfuscation {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	for _, k := range keys {
		if !awgKeys[k] {
			return "", fmt.Errorf("unknown AmneziaWG setting %q", k)
		}
		fmt.Fprintf(&b, "%s=%s\n", k, c.Obfuscation[k])
	}

	pub, err := keyHex(c.PeerPublicKey)
	if err != nil {
		return "", fmt.Errorf("peer public key: %w", err)
	}
	fmt.Fprintf(&b, "replace_peers=true\npublic_key=%s\n", pub)
	if c.PresharedKey != "" {
		psk, err := keyHex(c.PresharedKey)
		if err != nil {
			return "", fmt.Errorf("preshared key: %w", err)
		}
		fmt.Fprintf(&b, "preshared_key=%s\n", psk)
	}
	if c.Endpoint != "" {
		if _, err := netip.ParseAddrPort(c.Endpoint); err != nil {
			return "", fmt.Errorf("endpoint must be ip:port, got %q", c.Endpoint)
		}
		fmt.Fprintf(&b, "endpoint=%s\n", c.Endpoint)
	}
	if c.Keepalive > 0 {
		fmt.Fprintf(&b, "persistent_keepalive_interval=%d\n", c.Keepalive)
	}
	b.WriteString("replace_allowed_ips=true\n")
	for _, a := range c.AllowedIPs {
		p, err := netip.ParsePrefix(strings.TrimSpace(a))
		if err != nil {
			return "", fmt.Errorf("allowed IP %q: %w", a, err)
		}
		fmt.Fprintf(&b, "allowed_ip=%s\n", p)
	}
	return b.String(), nil
}

func parseAddrs(list []string) ([]netip.Addr, error) {
	out := make([]netip.Addr, 0, len(list))
	for _, s := range list {
		s = strings.TrimSpace(s)
		if p, err := netip.ParsePrefix(s); err == nil {
			out = append(out, p.Addr())
			continue
		}
		a, err := netip.ParseAddr(s)
		if err != nil {
			return nil, fmt.Errorf("address %q: %w", s, err)
		}
		out = append(out, a)
	}
	return out, nil
}

// Start brings the device up and opens the SOCKS5 proxy on a random loopback port.
func Start(c Config, logf func(string)) (*Tunnel, error) {
	addrs, err := parseAddrs(c.Addresses)
	if err != nil || len(addrs) == 0 {
		return nil, fmt.Errorf("the profile needs an [Interface] Address: %v", err)
	}
	dns, err := parseAddrs(c.DNS)
	if err != nil {
		return nil, err
	}
	mtu := c.MTU
	if mtu == 0 {
		mtu = 1420
	}
	cfg, err := uapi(c)
	if err != nil {
		return nil, err
	}

	tdev, tnet, err := netstack.CreateNetTUN(addrs, dns, mtu)
	if err != nil {
		return nil, fmt.Errorf("network stack: %w", err)
	}
	logger := &device.Logger{
		Verbosef: func(string, ...any) {},
		Errorf:   func(f string, a ...any) { logf(fmt.Sprintf(f, a...)) },
	}
	activity := &activityBind{Bind: conn.NewDefaultBind()}
	dev := device.NewDevice(tdev, activity, logger)
	if err := dev.IpcSet(cfg); err != nil {
		dev.Close()
		return nil, fmt.Errorf("applying profile: %w", err)
	}
	if err := dev.Up(); err != nil {
		dev.Close()
		return nil, err
	}

	// Connecting must initiate traffic even when PersistentKeepalive is explicitly
	// zero. SparkBox waits for a handshake before exposing SOCKS to SSH.
	if c.Endpoint != "" {
		raw, _ := base64.StdEncoding.DecodeString(strings.TrimSpace(c.PeerPublicKey))
		var key device.NoisePublicKey
		copy(key[:], raw)
		if peer := dev.LookupPeer(key); peer != nil {
			peer.SendKeepalive()
		}
	}
	t := &Tunnel{dev: dev, activity: activity, tnet: tnet, forwards: map[string]net.Listener{}}
	if c.SocksUser != "" {
		l, err := net.Listen("tcp", "127.0.0.1:0")
		if err != nil {
			dev.Close()
			return nil, err
		}
		t.socks = l
		go serveSocks(l, c.SocksUser, c.SocksPass, t.dial)
	}
	return t, nil
}

// ServerNet exposes the user-space network, for the test peer.
func (t *Tunnel) ServerNet() *netstack.Net { return t.tnet }

func (t *Tunnel) SocksAddr() string {
	if t.socks == nil {
		return ""
	}
	return t.socks.Addr().String()
}

func (t *Tunnel) dial(ctx context.Context, host string, port int) (net.Conn, error) {
	addr, err := netip.ParseAddr(host)
	if err != nil {
		ips, lerr := t.tnet.LookupContextHost(ctx, host)
		if lerr != nil || len(ips) == 0 {
			return nil, fmt.Errorf("can't resolve %s through the tunnel", host)
		}
		if addr, err = netip.ParseAddr(ips[0]); err != nil {
			return nil, err
		}
	}
	return t.tnet.DialContextTCPAddrPort(ctx, netip.AddrPortFrom(addr, uint16(port)))
}

// Probe opens a TCP connection through the tunnel and reports how long it took.
func (t *Tunnel) Probe(target string, timeout time.Duration) (time.Duration, error) {
	host, portStr, err := net.SplitHostPort(target)
	if err != nil {
		return 0, err
	}
	port, err := strconv.Atoi(portStr)
	if err != nil {
		return 0, err
	}
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	start := time.Now()
	c, err := t.dial(ctx, host, port)
	if err != nil {
		return 0, err
	}
	c.Close()
	return time.Since(start), nil
}

// Forward listens on a local address and relays every connection to target
// through the tunnel. It returns the address actually bound (listen may use port 0).
func (t *Tunnel) Forward(listen, target string) (string, error) {
	host, portStr, err := net.SplitHostPort(target)
	if err != nil {
		return "", err
	}
	port, err := strconv.Atoi(portStr)
	if err != nil {
		return "", err
	}
	l, err := net.Listen("tcp", listen)
	if err != nil {
		return "", err
	}
	bound := l.Addr().String()
	t.mu.Lock()
	t.forwards[bound] = l
	t.mu.Unlock()
	go func() {
		for {
			local, err := l.Accept()
			if err != nil {
				return
			}
			go func() {
				ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
				remote, err := t.dial(ctx, host, port)
				cancel()
				if err != nil {
					local.Close()
					return
				}
				pipe(local, remote)
			}()
		}
	}()
	return bound, nil
}

func (t *Tunnel) Unforward(listen string) {
	t.mu.Lock()
	defer t.mu.Unlock()
	if l, ok := t.forwards[listen]; ok {
		l.Close()
		delete(t.forwards, listen)
	}
}

// Stats is the peer's state from the device: seconds since the last handshake
// (-1 if none yet) and bytes moved.
type Stats struct {
	HandshakeAgeSec int64  `json:"handshakeAgeSec"`
	RxBytes         uint64 `json:"rxBytes"`
	TxBytes         uint64 `json:"txBytes"`
	ListenPort      int    `json:"-"`
	UDPSent         uint64 `json:"udpSent"`
	UDPReceived     uint64 `json:"udpReceived"`
	UDPSendErrors   uint64 `json:"udpSendErrors"`
}

func (t *Tunnel) Stats() Stats {
	s := Stats{HandshakeAgeSec: -1, UDPSent: t.activity.sent.Load(), UDPReceived: t.activity.received.Load(), UDPSendErrors: t.activity.sendErrors.Load()}
	raw, err := t.dev.IpcGet()
	if err != nil {
		return s
	}
	var hsSec int64
	for _, line := range strings.Split(raw, "\n") {
		k, v, ok := strings.Cut(line, "=")
		if !ok {
			continue
		}
		switch k {
		case "last_handshake_time_sec":
			hsSec, _ = strconv.ParseInt(v, 10, 64)
		case "rx_bytes":
			s.RxBytes, _ = strconv.ParseUint(v, 10, 64)
		case "tx_bytes":
			s.TxBytes, _ = strconv.ParseUint(v, 10, 64)
		case "listen_port":
			s.ListenPort, _ = strconv.Atoi(v)
		}
	}
	if hsSec > 0 {
		s.HandshakeAgeSec = time.Now().Unix() - hsSec
	}
	return s
}

func (t *Tunnel) Close() {
	t.mu.Lock()
	for _, l := range t.forwards {
		l.Close()
	}
	t.mu.Unlock()
	if t.socks != nil {
		t.socks.Close()
	}
	t.dev.Close()
}

func pipe(a, b net.Conn) {
	var wg sync.WaitGroup
	wg.Add(2)
	cp := func(dst, src net.Conn) {
		defer wg.Done()
		io.Copy(dst, src)
		if cw, ok := dst.(interface{ CloseWrite() error }); ok {
			cw.CloseWrite()
		} else {
			dst.Close()
		}
	}
	go cp(a, b)
	go cp(b, a)
	wg.Wait()
	a.Close()
	b.Close()
}
