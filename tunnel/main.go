package main

// Process protocol with SparkBox (all JSON, one object per line):
//   stdin, first line:  Config
//   stdin, afterwards:  {"cmd":"probe","id":"…","target":"192.168.50.51:22"}
//                       {"cmd":"forward","id":"…","listen":"127.0.0.1:8000","target":"192.168.50.51:8000"}
//                       {"cmd":"unforward","listen":"127.0.0.1:8000"}
//                       {"cmd":"stop"}
//   stdout events:      ready · stats (every 2 s) · probe · forward · error · log
// Closing stdin also stops the tunnel, so the helper never outlives SparkBox.

import (
	"bufio"
	"encoding/json"
	"os"
	"sync"
	"time"
)

type command struct {
	Cmd    string `json:"cmd"`
	ID     string `json:"id"`
	Target string `json:"target"`
	Listen string `json:"listen"`
}

var outMu sync.Mutex

func emit(v map[string]any) {
	outMu.Lock()
	defer outMu.Unlock()
	b, _ := json.Marshal(v)
	os.Stdout.Write(append(b, '\n'))
}

func main() {
	in := bufio.NewScanner(os.Stdin)
	in.Buffer(make([]byte, 64*1024), 1024*1024)
	if !in.Scan() {
		return
	}
	var cfg Config
	if err := json.Unmarshal(in.Bytes(), &cfg); err != nil {
		emit(map[string]any{"event": "error", "message": "bad config: " + err.Error()})
		os.Exit(2)
	}

	t, err := Start(cfg, func(msg string) { emit(map[string]any{"event": "log", "message": msg}) })
	if err != nil {
		emit(map[string]any{"event": "error", "message": err.Error()})
		os.Exit(1)
	}
	defer t.Close()
	emit(map[string]any{"event": "ready", "socks": t.SocksAddr()})

	go func() {
		for range time.Tick(2 * time.Second) {
			s := t.Stats()
			emit(map[string]any{"event": "stats", "handshakeAgeSec": s.HandshakeAgeSec, "rxBytes": s.RxBytes, "txBytes": s.TxBytes, "udpSent": s.UDPSent, "udpReceived": s.UDPReceived, "udpSendErrors": s.UDPSendErrors})
		}
	}()

	for in.Scan() {
		var c command
		if json.Unmarshal(in.Bytes(), &c) != nil {
			continue
		}
		switch c.Cmd {
		case "probe":
			go func(c command) {
				d, err := t.Probe(c.Target, 4*time.Second)
				ev := map[string]any{"event": "probe", "id": c.ID, "ok": err == nil, "ms": float64(d.Microseconds()) / 1000}
				if err != nil {
					ev["error"] = err.Error()
				}
				emit(ev)
			}(c)
		case "forward":
			bound, err := t.Forward(c.Listen, c.Target)
			ev := map[string]any{"event": "forward", "id": c.ID, "ok": err == nil, "listen": bound}
			if err != nil {
				ev["error"] = err.Error()
			}
			emit(ev)
		case "unforward":
			t.Unforward(c.Listen)
		case "stop":
			return
		}
	}
}
