package httpapi

import (
	"context"
	"crypto/tls"
	"fmt"
	"net"
	"net/http"
	"strings"
	"time"
)

type Listener struct {
	net.Listener
	TLS bool
}

func OpenListener(address, certificatePath, keyPath string) (*Listener, error) {
	tlsEnabled, err := validateListenerSecurity(address, certificatePath, keyPath)
	if err != nil {
		return nil, err
	}
	listener, err := net.Listen("tcp", address)
	if err != nil {
		return nil, fmt.Errorf("listen on %s: %w", address, err)
	}
	if tcpAddress, ok := listener.Addr().(*net.TCPAddr); ok && !tcpAddress.IP.IsLoopback() && !tlsEnabled {
		listener.Close()
		return nil, fmt.Errorf("plain HTTP may listen only on loopback addresses")
	}
	if !tlsEnabled {
		return &Listener{Listener: listener}, nil
	}
	certificate, err := tls.LoadX509KeyPair(certificatePath, keyPath)
	if err != nil {
		listener.Close()
		return nil, fmt.Errorf("load TLS certificate and key: %w", err)
	}
	configuration := &tls.Config{
		MinVersion:   tls.VersionTLS12,
		Certificates: []tls.Certificate{certificate},
	}
	return &Listener{Listener: tls.NewListener(listener, configuration), TLS: true}, nil
}

func validateListenerSecurity(address, certificatePath, keyPath string) (bool, error) {
	certificateConfigured := certificatePath != ""
	keyConfigured := keyPath != ""
	if certificateConfigured != keyConfigured {
		return false, fmt.Errorf("--tls-cert and --tls-key must be supplied together")
	}
	host, _, err := net.SplitHostPort(address)
	if err != nil {
		return false, fmt.Errorf("listen address must use host:port form: %w", err)
	}
	if host == "" {
		host = "0.0.0.0"
	}
	loopbackOnly, err := resolvesOnlyToLoopback(host)
	if err != nil {
		return false, err
	}
	if !loopbackOnly && !certificateConfigured {
		return false, fmt.Errorf("every non-loopback listener requires --tls-cert and --tls-key")
	}
	return certificateConfigured, nil
}

func resolvesOnlyToLoopback(host string) (bool, error) {
	host = strings.Trim(host, "[]")
	if ip := net.ParseIP(host); ip != nil {
		return ip.IsLoopback(), nil
	}
	addresses, err := net.LookupIP(host)
	if err != nil {
		return false, fmt.Errorf("resolve listen host %q: %w", host, err)
	}
	if len(addresses) == 0 {
		return false, fmt.Errorf("listen host %q resolved to no addresses", host)
	}
	for _, address := range addresses {
		if !address.IsLoopback() {
			return false, nil
		}
	}
	return true, nil
}

func Serve(ctx context.Context, server *http.Server, listener *Listener) error {
	shutdownDone := make(chan struct{})
	serveDone := make(chan struct{})
	go func() {
		defer close(shutdownDone)
		select {
		case <-ctx.Done():
			shutdownContext, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			defer cancel()
			_ = server.Shutdown(shutdownContext)
		case <-serveDone:
		}
	}()
	err := server.Serve(listener)
	close(serveDone)
	if err == http.ErrServerClosed {
		err = nil
	}
	<-shutdownDone
	return err
}
