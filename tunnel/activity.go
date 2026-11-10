package main

import (
	"github.com/amnezia-vpn/amneziawg-go/conn"
	"sync/atomic"
)

// Counts outer UDP traffic, including handshakes. Device byte counters only
// count authenticated traffic, so zero there cannot explain a failed handshake.
// No packet contents, addresses or keys are recorded.
type activityBind struct {
	conn.Bind
	sent       atomic.Uint64
	received   atomic.Uint64
	sendErrors atomic.Uint64
}

func (b *activityBind) Send(bufs [][]byte, ep conn.Endpoint) error {
	err := b.Bind.Send(bufs, ep)
	if err != nil {
		b.sendErrors.Add(1)
	} else {
		b.sent.Add(uint64(len(bufs)))
	}
	return err
}

func (b *activityBind) Open(port uint16) ([]conn.ReceiveFunc, uint16, error) {
	fns, port, err := b.Bind.Open(port)
	for i, receive := range fns {
		fns[i] = func(packets [][]byte, sizes []int, eps []conn.Endpoint) (int, error) {
			n, err := receive(packets, sizes, eps)
			for _, size := range sizes[:n] {
				if size > 0 {
					b.received.Add(1)
				}
			}
			return n, err
		}
	}
	return fns, port, err
}
