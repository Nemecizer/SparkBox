//! Credential-free RDP TLS negotiation from this Mac. The certificate must match
//! the public certificate just read over the node's authenticated SSH connection.
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug)]
struct Pin {
    digest: String,
    provider: Arc<rustls::crypto::CryptoProvider>,
}
impl ServerCertVerifier for Pin {
    fn verify_server_cert(
        &self,
        cert: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if crate::live::sha256_hex(cert.as_ref()) != self.digest {
            return Err(rustls::Error::General(
                "RDP certificate does not match the trusted SSH reading".into(),
            ));
        }
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        msg: &[u8],
        cert: &CertificateDer<'_>,
        sig: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            msg,
            cert,
            sig,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        msg: &[u8],
        cert: &CertificateDer<'_>,
        sig: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            msg,
            cert,
            sig,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
const REQUEST: [u8; 19] = [3, 0, 0, 19, 14, 224, 0, 0, 0, 0, 0, 1, 0, 8, 0, 1, 0, 0, 0];
fn accepted(reply: &[u8; 19]) -> bool {
    reply[..5] == [3, 0, 0, 19, 14]
        && reply[5] == 208
        && reply[11] == 2
        && reply[13..] == [8, 0, 1, 0, 0, 0]
}

pub async fn probe(host: &str, port: u16, pin: &str) -> Result<String, String> {
    if !(3389..=3399).contains(&port)
        || pin.len() != 64
        || !pin.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("Invalid RDP port or certificate reading".into());
    }
    exchange(host, port, pin).await
}

async fn exchange(host: &str, port: u16, pin: &str) -> Result<String, String> {
    tokio::time::timeout(Duration::from_secs(8), async {
        let mut socket = tokio::net::TcpStream::connect((host, port)).await.map_err(|e| e.to_string())?;
        socket.write_all(&REQUEST).await.map_err(|e| e.to_string())?;
        let mut reply = [0; 19];
        socket.read_exact(&mut reply).await.map_err(|e| e.to_string())?;
        if !accepted(&reply) { return Err("The server did not accept RDP with TLS".into()); }
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = rustls::ClientConfig::builder_with_provider(provider.clone()).with_safe_default_protocol_versions().map_err(|e| e.to_string())?
            .dangerous().with_custom_certificate_verifier(Arc::new(Pin { digest: pin.to_lowercase(), provider })).with_no_client_auth();
        let name = ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        let mut tls = connector.connect(name, socket).await.map_err(|e| e.to_string())?;
        let protocol = format!("{:?}", tls.get_ref().1.protocol_version().ok_or("Missing TLS version")?);
        let _ = tls.shutdown().await;
        Ok(format!("Encrypted RDP connection passed ({protocol}); certificate matches the node. Desktop login has not been tested."))
    }).await.map_err(|_| "RDP connection from this Mac timed out".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn negotiation_rejects_plain_rdp_failure_and_unrelated_protocols() {
        let mut r = [3, 0, 0, 19, 14, 208, 0, 0, 0, 0, 0, 2, 0, 8, 0, 1, 0, 0, 0];
        assert!(accepted(&r));
        r[12] = 1;
        assert!(accepted(&r));
        r[15] = 0;
        assert!(!accepted(&r));
        r[15] = 2;
        assert!(!accepted(&r));
        r[15] = 1;
        r[11] = 3;
        assert!(!accepted(&r));
    }
    #[test]
    fn unrelated_certificate_cannot_pass() {
        let p = Pin {
            digest: "0".repeat(64),
            provider: Arc::new(rustls::crypto::aws_lc_rs::default_provider()),
        };
        assert!(p
            .verify_server_cert(
                &CertificateDer::from(vec![1, 2, 3]),
                &[],
                &ServerName::try_from("localhost").unwrap(),
                &[],
                UnixTime::now()
            )
            .is_err());
    }
    #[tokio::test]
    async fn plain_open_socket_does_not_pass_as_rdp() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut req = [0; 19];
            s.read_exact(&mut req).await.unwrap();
            s.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.unwrap();
        });
        assert!(exchange("127.0.0.1", port, &"f".repeat(64))
            .await
            .unwrap_err()
            .contains("did not accept"));
        server.await.unwrap();
    }
    #[tokio::test]
    async fn loopback_rdp_tls_requires_the_exact_authenticated_certificate() {
        use std::{fs, process::Command};
        let dir = crate::fake_node::temp("rdp-tls");
        fs::create_dir_all(&dir).unwrap();
        let openssl = |args: &[&str]| {
            let out = Command::new("openssl")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        openssl(&[
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "2",
            "-subj",
            "/CN=localhost",
            "-keyout",
            "key.pem",
            "-out",
            "cert.pem",
        ]);
        openssl(&[
            "x509", "-in", "cert.pem", "-outform", "DER", "-out", "cert.der",
        ]);
        openssl(&[
            "pkcs8", "-topk8", "-nocrypt", "-in", "key.pem", "-outform", "DER", "-out", "key.der",
        ]);
        let cert = fs::read(dir.join("cert.der")).unwrap();
        let key =
            rustls::pki_types::PrivatePkcs8KeyDer::from(fs::read(dir.join("key.der")).unwrap());
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![CertificateDer::from(cert.clone())], key.into())
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        for pin in [crate::live::sha256_hex(&cert), "0".repeat(64)] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let a = acceptor.clone();
            let server = tokio::spawn(async move {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut req = [0; 19];
                s.read_exact(&mut req).await.unwrap();
                assert_eq!(req, REQUEST);
                // Capability flag 1 is legal and must not make the server look broken.
                s.write_all(&[3, 0, 0, 19, 14, 208, 0, 0, 0, 0, 0, 2, 1, 8, 0, 1, 0, 0, 0])
                    .await
                    .unwrap();
                if let Ok(mut tls) = a.accept(s).await {
                    let mut b = [0];
                    let _ = tls.read(&mut b).await;
                }
            });
            let result = exchange("127.0.0.1", port, &pin).await;
            assert_eq!(result.is_ok(), pin != "0".repeat(64), "{result:?}");
            server.await.unwrap();
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
