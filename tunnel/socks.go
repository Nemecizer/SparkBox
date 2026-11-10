package main

import (
	"context"
	"crypto/subtle"
	"encoding/binary"
	"io"
	"net"
	"time"
)

// A minimal SOCKS5 server (RFC 1928 CONNECT + RFC 1929 username/password).
// Only SparkBox knows the random credentials, so other local apps can't ride
// the tunnel through this port.

type dialFunc func(ctx context.Context, host string, port int) (net.Conn, error)

func serveSocks(l net.Listener, user, pass string, dial dialFunc) {
	for {
		c, err := l.Accept()
		if err != nil {
			return
		}
		go handleSocks(c, user, pass, dial)
	}
}

func handleSocks(c net.Conn, user, pass string, dial dialFunc) {
	c.SetDeadline(time.Now().Add(15 * time.Second))
	remote, err := socksHandshake(c, user, pass, dial)
	if err != nil {
		c.Close()
		return
	}
	c.SetDeadline(time.Time{})
	pipe(c, remote)
}

func socksHandshake(c net.Conn, user, pass string, dial dialFunc) (net.Conn, error) {
	buf := make([]byte, 262)

	// Greeting: VER NMETHODS METHODS...
	if _, err := io.ReadFull(c, buf[:2]); err != nil || buf[0] != 5 {
		return nil, errBadRequest
	}
	methods := buf[2 : 2+int(buf[1])]
	if _, err := io.ReadFull(c, methods); err != nil {
		return nil, err
	}
	hasUserPass := false
	for _, m := range methods {
		if m == 2 {
			hasUserPass = true
		}
	}
	if !hasUserPass {
		c.Write([]byte{5, 0xFF})
		return nil, errBadRequest
	}
	c.Write([]byte{5, 2})

	// Username/password: VER ULEN UNAME PLEN PASSWD
	if _, err := io.ReadFull(c, buf[:2]); err != nil || buf[0] != 1 {
		return nil, errBadRequest
	}
	u := make([]byte, buf[1])
	if _, err := io.ReadFull(c, u); err != nil {
		return nil, err
	}
	if _, err := io.ReadFull(c, buf[:1]); err != nil {
		return nil, err
	}
	p := make([]byte, buf[0])
	if _, err := io.ReadFull(c, p); err != nil {
		return nil, err
	}
	userOK := subtle.ConstantTimeCompare(u, []byte(user)) == 1
	passOK := subtle.ConstantTimeCompare(p, []byte(pass)) == 1
	if !userOK || !passOK {
		c.Write([]byte{1, 1})
		return nil, errBadRequest
	}
	c.Write([]byte{1, 0})

	// Request: VER CMD RSV ATYP DST.ADDR DST.PORT
	if _, err := io.ReadFull(c, buf[:4]); err != nil || buf[0] != 5 {
		return nil, errBadRequest
	}
	if buf[1] != 1 { // CONNECT only
		reply(c, 7)
		return nil, errBadRequest
	}
	var host string
	switch buf[3] {
	case 1:
		ip := make([]byte, 4)
		if _, err := io.ReadFull(c, ip); err != nil {
			return nil, err
		}
		host = net.IP(ip).String()
	case 4:
		ip := make([]byte, 16)
		if _, err := io.ReadFull(c, ip); err != nil {
			return nil, err
		}
		host = net.IP(ip).String()
	case 3:
		if _, err := io.ReadFull(c, buf[:1]); err != nil {
			return nil, err
		}
		name := make([]byte, buf[0])
		if _, err := io.ReadFull(c, name); err != nil {
			return nil, err
		}
		host = string(name)
	default:
		reply(c, 8)
		return nil, errBadRequest
	}
	if _, err := io.ReadFull(c, buf[:2]); err != nil {
		return nil, err
	}
	port := int(binary.BigEndian.Uint16(buf[:2]))

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	remote, err := dial(ctx, host, port)
	if err != nil {
		reply(c, 4) // host unreachable
		return nil, err
	}
	reply(c, 0)
	return remote, nil
}

func reply(c net.Conn, code byte) {
	c.Write([]byte{5, code, 0, 1, 0, 0, 0, 0, 0, 0})
}

type socksError string

func (e socksError) Error() string { return string(e) }

const errBadRequest = socksError("bad SOCKS request")
