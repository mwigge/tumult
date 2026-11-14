//! In-memory connections and TLS material for tests of the listener and
//! of the router behind it.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{ready, Context, Poll};

use tokio::io::DuplexStream;
use tokio::sync::mpsc;
use tokio_rustls::rustls;

use super::{Accept, TlsConfig};

/// A self-signed certificate for `localhost`, valid until 2126, and its
/// key. Test material only.
pub(crate) const CERT_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIBkzCCATigAwIBAgIUV5d+A5IchQED5lsdSaln9OgDvYUwCgYIKoZIzj0EAwIw
FDESMBAGA1UEAwwJbG9jYWxob3N0MCAXDTI2MTExMzIyMjg1MFoYDzIxMjYxMDIw
MjIyODUwWjAUMRIwEAYDVQQDDAlsb2NhbGhvc3QwWTATBgcqhkjOPQIBBggqhkjO
PQMBBwNCAATk8+CZpWzN4qurvjgDFwx1+mfl39zNcApbONrjg7vllmcms1THXVak
CO+C7keoTOT/4/iU7KRvdpggsjmr85/To2YwZDAdBgNVHQ4EFgQUunMxwUUBqvKV
MN184GRoL5b1vQ4wHwYDVR0jBBgwFoAUunMxwUUBqvKVMN184GRoL5b1vQ4wFAYD
VR0RBA0wC4IJbG9jYWxob3N0MAwGA1UdEwEB/wQCMAAwCgYIKoZIzj0EAwIDSQAw
RgIhAOS/bUiv0yGD4Pxb1PKS4EuuKigYlU63Et/c/cCDzN1BAiEAsjyu4cD/rOTe
Dg2Z0iprmD/PAnqqBe1X6rLeEubDzl0=
-----END CERTIFICATE-----
";
pub(crate) const KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgrCcSpK4PJ/OyBCsN
+vDbaSdJ9nNtDAJ6Q2bjZfSeZHKhRANCAATk8+CZpWzN4qurvjgDFwx1+mfl39zN
cApbONrjg7vllmcms1THXVakCO+C7keoTOT/4/iU7KRvdpggsjmr85/T
-----END PRIVATE KEY-----
";

/// The listener's TLS settings for [`CERT_PEM`].
pub(crate) fn server_tls() -> Arc<TlsConfig> {
    super::tls_config(CERT_PEM.as_bytes(), KEY_PEM.as_bytes()).unwrap()
}

/// A TLS client that trusts [`CERT_PEM`] and speaks HTTP/2.
pub(crate) fn client_tls() -> Arc<rustls::ClientConfig> {
    use rustls::pki_types::{pem::PemObject, CertificateDer};

    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(CERT_PEM.as_bytes()).unwrap())
        .unwrap();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols.push(b"h2".to_vec());
    Arc::new(config)
}

/// The `ClientHello` of a real TLS 1.3 client, with the `early_data`
/// extension added: the server turns the early data down and, as the
/// protocol says, skips the records it cannot decrypt.
pub(crate) fn hello_offering_early_data() -> Vec<u8> {
    let name = "localhost".try_into().unwrap();
    let mut client = rustls::ClientConnection::new(client_tls(), name).unwrap();
    let mut hello = Vec::new();
    client.write_tls(&mut hello).unwrap();

    // Record header (5), handshake header (4), version (2), random (32),
    // then session id, cipher suites and compression methods, each behind
    // its length; the extensions are the rest.
    let mut at = 5 + 4 + 2 + 32;
    at += 1 + usize::from(hello[at]);
    at += 2 + usize::from(u16::from_be_bytes([hello[at], hello[at + 1]]));
    at += 1 + usize::from(hello[at]);
    let grow = |bytes: &mut [u8]| {
        let len = u16::from_be_bytes([bytes[0], bytes[1]]) + 4;
        bytes.copy_from_slice(&len.to_be_bytes());
    };
    grow(&mut hello[at..at + 2]);
    grow(&mut hello[7..9]);
    grow(&mut hello[3..5]);
    // early_data (42), empty.
    hello.extend_from_slice(&[0, 42, 0, 0]);
    hello
}

/// A TLS application-data record of `len` bytes that decrypt to nothing.
pub(crate) fn undecryptable_record(len: u8) -> Vec<u8> {
    let mut record = vec![0x17, 3, 3, 0, len];
    record.resize(5 + usize::from(len), 0xab);
    record
}

/// `ip` as the address a connection came from.
pub(crate) fn from(ip: &str) -> SocketAddr {
    SocketAddr::new(ip.parse::<IpAddr>().unwrap(), 40_000)
}

type Accepted = io::Result<(DuplexStream, SocketAddr)>;

/// The client side of an in-memory listener.
pub(crate) struct Dial(mpsc::UnboundedSender<Accepted>);

/// The in-memory listener [`Dial`] connects to.
pub(crate) struct Dialled {
    accepted: mpsc::UnboundedReceiver<Accepted>,
    polls: Arc<AtomicUsize>,
}

/// An in-memory listener and the way to connect to it.
pub(crate) fn dialler() -> (Dial, Dialled) {
    let (tx, accepted) = mpsc::unbounded_channel();
    let polls = Arc::default();
    (Dial(tx), Dialled { accepted, polls })
}

impl Dial {
    /// Connect from `remote`; the client's end.
    pub(crate) fn connect(&self, remote: SocketAddr) -> DuplexStream {
        let (client, server) = tokio::io::duplex(1 << 16);
        self.0.send(Ok((server, remote))).expect("listener gone");
        client
    }

    /// Make the next accept fail with `error`.
    pub(crate) fn fail(&self, error: io::Error) {
        self.0.send(Err(error)).expect("listener gone");
    }
}

impl Dialled {
    /// Counts how often the listener is asked for a connection.
    pub(crate) fn polls(&self) -> Arc<AtomicUsize> {
        self.polls.clone()
    }
}

impl Accept for Dialled {
    type Io = DuplexStream;

    fn poll_accept(&mut self, cx: &mut Context<'_>) -> Poll<Accepted> {
        self.polls.fetch_add(1, Ordering::Relaxed);
        match ready!(self.accepted.poll_recv(cx)) {
            Some(accepted) => Poll::Ready(accepted),
            // Every `Dial` is gone: nobody connects any more.
            None => Poll::Pending,
        }
    }
}
