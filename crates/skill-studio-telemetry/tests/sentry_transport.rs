use sentry::{Client, ClientOptions, Hub, Scope, Transport};
use sentry_types::protocol::latest::*;
use skill_studio_telemetry::*;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

fn identity() -> TelemetryIdentity {
    TelemetryIdentity::new(TelemetrySurface::Cli, TelemetryEnvironment::Test, (1, 2, 3))
}
fn input() -> Envelope {
    let mut transaction = Transaction {
        name: Some("skill.scan".into()),
        ..Default::default()
    };
    transaction
        .tags
        .insert("private".into(), "PRIVATE_SECRET /Users/private".into());
    let mut envelope = Envelope::new();
    envelope.add_item(transaction);
    envelope
}

struct Server {
    address: SocketAddr,
    request: mpsc::Receiver<Vec<u8>>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}
fn server(response: Option<String>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, request) = mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let start = Instant::now();
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if start.elapsed() > Duration::from_secs(4) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("{error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0; 4096];
            let read = stream.read(&mut chunk).unwrap();
            assert_ne!(read, 0);
            bytes.extend_from_slice(&chunk[..read]);
            assert!(bytes.len() < 300_000);
            if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                let length: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                if bytes.len() >= end + 4 + length {
                    break;
                }
            }
        }
        sender.send(bytes).unwrap();
        if let Some(response) = response {
            stream.write_all(response.as_bytes()).unwrap();
        } else {
            std::thread::sleep(Duration::from_millis(2300));
        }
    });
    Server {
        address,
        request,
        worker: Some(worker),
    }
}

#[test]
fn actual_http_transport_sends_only_sanitized_envelopes_with_sentry_headers() {
    let server = server(Some(
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".into(),
    ));
    let dsn = format!("http://public@{}/1", server.address)
        .parse()
        .unwrap();
    let transport = SentryTransport::http(&dsn, identity()).unwrap();
    transport.send_envelope(input());
    assert!(transport.flush(Duration::from_secs(2)));
    let bytes = server.request.recv_timeout(Duration::from_secs(1)).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(
        text.starts_with("POST /api/1/envelope/ HTTP/1.1\r\n"),
        "{text}"
    );
    assert!(text
        .to_ascii_lowercase()
        .contains("content-type: application/x-sentry-envelope"));
    assert!(text.to_ascii_lowercase().contains("x-sentry-auth:"));
    assert!(text.contains("sentry_key=public"));
    assert!(!text.contains("PRIVATE_SECRET"));
    assert!(!text.contains("/Users/"));
    assert!(transport.shutdown(Duration::from_secs(1)));
    assert_eq!(transport.stats().export.delivered, 1);
}

#[test]
fn redirects_are_not_followed_and_rejection_is_counted() {
    let trap = TcpListener::bind("127.0.0.1:0").unwrap();
    trap.set_nonblocking(true).unwrap();
    let server=server(Some(format!("HTTP/1.1 302 Found\r\nLocation: http://{}/leak\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",trap.local_addr().unwrap())));
    let dsn = format!("http://public@{}/1", server.address)
        .parse()
        .unwrap();
    let transport = SentryTransport::http(&dsn, identity()).unwrap();
    transport.send_envelope(input());
    assert!(!transport.shutdown(Duration::from_secs(2)));
    assert_eq!(transport.stats().export.failed, 1);
    assert_eq!(
        trap.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn insecure_remote_and_secret_dsns_are_rejected_before_starting_transport() {
    for (dsn, error) in [
        (
            "http://public@example.invalid/1",
            TransportSetupError::InsecureEndpoint,
        ),
        (
            "https://public:private@example.invalid/1",
            TransportSetupError::SecretDsn,
        ),
    ] {
        assert!(
            matches!(SentryTransport::http(&dsn.parse().unwrap(),identity()),Err(actual) if actual == error)
        );
    }
}

#[test]
fn sdk_uses_the_same_final_boundary_and_can_flush_more_than_once() {
    let (sent, received) = mpsc::channel();
    let transport = Arc::new(
        SentryTransport::with_sink(identity(), move |bytes| {
            sent.send(bytes.to_vec()).unwrap();
            Ok(())
        })
        .unwrap(),
    );
    let client = Arc::new(Client::from(
        ClientOptions::new()
            .dsn("https://public@example.invalid/1")
            .default_integrations(false)
            .transport(transport.clone())
            .traces_sample_rate(1.0),
    ));
    let hub = Arc::new(Hub::new(Some(client.clone()), Arc::new(Scope::default())));
    for _ in 0..2 {
        Hub::run(hub.clone(), || {
            sentry::configure_scope(|scope| scope.set_tag("private", "PRIVATE_SECRET"));
            sentry::start_transaction(sentry::TransactionContext::new("skill.scan", "skill.scan"))
                .finish();
        });
        assert!(client.flush(Some(Duration::from_secs(1))));
        let bytes = received.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(!String::from_utf8(bytes).unwrap().contains("PRIVATE_SECRET"));
    }
    transport.send_envelope(Envelope::from_bytes_raw(b"{}\nPRIVATE_SECRET".to_vec()).unwrap());
    assert_eq!(transport.stats().rejected_envelopes, 1);
    assert!(client.close(Some(Duration::from_secs(1))));
    assert_eq!(transport.stats().export.delivered, 2);
}

#[test]
fn silent_http_peer_times_out_without_holding_up_flush() {
    let server = server(None);
    let dsn = format!("http://public@{}/1", server.address)
        .parse()
        .unwrap();
    let transport = SentryTransport::http(&dsn, identity()).unwrap();
    transport.send_envelope(input());
    server.request.recv_timeout(Duration::from_secs(1)).unwrap();
    let start = Instant::now();
    assert!(!transport.flush(Duration::from_millis(20)));
    assert!(start.elapsed() < Duration::from_secs(1));
    while transport.stats().export.failed == 0 {
        assert!(start.elapsed() < Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(transport.stats().export.failed, 1);
    assert_eq!(transport.stats().export.delivered, 0);
    assert!(!transport.shutdown(Duration::from_secs(1)));
}
