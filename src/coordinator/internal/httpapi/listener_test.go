package httpapi

import (
	"context"
	"crypto/rand"
	"crypto/rsa"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/pem"
	"io"
	"math/big"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestListenerSecurityRules(t *testing.T) {
	for _, address := range []string{"127.0.0.1:8787", "127.0.0.2:8787", "[::1]:8787"} {
		tlsEnabled, err := validateListenerSecurity(address, "", "")
		if err != nil || tlsEnabled {
			t.Fatalf("loopback %s: tls=%v err=%v", address, tlsEnabled, err)
		}
	}
	for _, address := range []string{"0.0.0.0:8787", "[::]:8787", "192.168.1.8:8787", "100.64.0.8:8787"} {
		if _, err := validateListenerSecurity(address, "", ""); err == nil {
			t.Fatalf("non-loopback plaintext accepted: %s", address)
		}
		if tlsEnabled, err := validateListenerSecurity(address, "cert.pem", "key.pem"); err != nil || !tlsEnabled {
			t.Fatalf("TLS address %s: tls=%v err=%v", address, tlsEnabled, err)
		}
	}
	if _, err := validateListenerSecurity("127.0.0.1:8787", "cert.pem", ""); err == nil {
		t.Fatal("partial TLS configuration accepted")
	}
}

func TestNativeTLSListenerWithIPCertificate(t *testing.T) {
	certificatePath, keyPath, roots := testIPCertificate(t)
	listener, err := OpenListener("127.0.0.1:0", certificatePath, keyPath)
	if err != nil {
		t.Fatal(err)
	}
	if !listener.TLS {
		t.Fatal("TLS listener was not enabled")
	}
	ctx, cancel := context.WithCancel(context.Background())
	server := &http.Server{
		Handler: http.HandlerFunc(func(writer http.ResponseWriter, _ *http.Request) {
			_, _ = io.WriteString(writer, "ok")
		}),
		ReadHeaderTimeout: time.Second,
	}
	done := make(chan error, 1)
	go func() { done <- Serve(ctx, server, listener) }()
	client := &http.Client{Transport: &http.Transport{TLSClientConfig: testTLSConfig(roots)}}
	response, err := client.Get("https://" + listener.Addr().String())
	if err != nil {
		cancel()
		t.Fatal(err)
	}
	body, err := io.ReadAll(response.Body)
	response.Body.Close()
	if err != nil || string(body) != "ok" {
		t.Fatalf("TLS response = %q, %v", body, err)
	}
	if response.TLS == nil || response.TLS.Version < 0x0303 {
		t.Fatalf("TLS state = %#v", response.TLS)
	}
	cancel()
	if err := <-done; err != nil {
		t.Fatal(err)
	}
}

func testTLSConfig(roots *x509.CertPool) *tls.Config {
	return &tls.Config{RootCAs: roots, MinVersion: tls.VersionTLS12}
}

func testIPCertificate(t *testing.T) (string, string, *x509.CertPool) {
	t.Helper()
	privateKey, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		t.Fatal(err)
	}
	now := time.Now()
	template := x509.Certificate{
		SerialNumber:          big.NewInt(1),
		Subject:               pkix.Name{CommonName: "mini-sub2api test"},
		NotBefore:             now.Add(-time.Minute),
		NotAfter:              now.Add(time.Hour),
		KeyUsage:              x509.KeyUsageDigitalSignature | x509.KeyUsageKeyEncipherment | x509.KeyUsageCertSign,
		ExtKeyUsage:           []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
		IsCA:                  true,
		BasicConstraintsValid: true,
		IPAddresses:           []net.IP{net.ParseIP("127.0.0.1")},
	}
	certificateDER, err := x509.CreateCertificate(rand.Reader, &template, &template, &privateKey.PublicKey, privateKey)
	if err != nil {
		t.Fatal(err)
	}
	directory := t.TempDir()
	certificatePath := filepath.Join(directory, "server.crt")
	keyPath := filepath.Join(directory, "server.key")
	certificatePEM := pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: certificateDER})
	privateKeyPEM := pem.EncodeToMemory(&pem.Block{
		Type: "RSA PRIVATE KEY", Bytes: x509.MarshalPKCS1PrivateKey(privateKey),
	})
	if err := os.WriteFile(certificatePath, certificatePEM, 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(keyPath, privateKeyPEM, 0o600); err != nil {
		t.Fatal(err)
	}
	roots := x509.NewCertPool()
	if !roots.AppendCertsFromPEM(certificatePEM) {
		t.Fatal("append test root")
	}
	return certificatePath, keyPath, roots
}
