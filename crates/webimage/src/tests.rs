//! A port of `internal/webimage/webimage_test.go`, plus the limits the Go client got from `net/http`.

use super::*;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::AtomicUsize;

const PUBLIC: &str = "93.184.216.34";

/// A small HTTP/1.1 fixture. `handle` gets the raw request and writes the raw response.
fn server(handle: impl Fn(&str, &mut TcpStream) + Send + Sync + 'static) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = Arc::new(handle);
    std::thread::spawn(move || {
        for mut s in listener.incoming().flatten() {
            let handle = handle.clone();
            std::thread::spawn(move || {
                let request = read_request(&mut s);
                handle(&request, &mut s);
            });
        }
    });
    addr
}

fn read_request(s: &mut impl Read) -> String {
    let mut got = Vec::new();
    let mut byte = [0u8; 1];
    while !got.ends_with(b"\r\n\r\n") {
        match s.read(&mut byte) {
            Ok(1) => got.push(byte[0]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&got).into_owned()
}

fn path_of(request: &str) -> &str {
    request.split_whitespace().nth(1).unwrap_or("")
}

fn respond(s: &mut impl Write, status: &str, headers: &str, body: &[u8]) {
    let head = format!("HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(body);
}

/// Resolves everything to a public address and connects to the fixture, as `useTestClient` does.
fn client_for(fixture: SocketAddr) -> Client {
    Client::with_parts(
        Box::new(|uri, _, _| Ok(vec![SocketAddr::new(PUBLIC.parse().unwrap(), uri.port_u16().unwrap_or(80))])),
        Some(Box::new(move |_| Ok(fixture))),
        ureq::tls::TlsConfig::default(),
        Duration::from_secs(5),
    )
}

fn flag() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

fn ips(list: &[&str]) -> Vec<IpAddr> {
    list.iter().map(|s| s.parse().unwrap()).collect()
}

#[test]
fn check_table() {
    for (raw, ok) in [
        ("https://example.com/a.png", true),
        ("//example.com/a.png", true),
        ("http://192.168.1.10/a.png", false),
        ("file:///etc/passwd", false),
        ("ftp://example.com/a.png", false),
        ("http://localhost/a.png", false),
        ("http://app.localhost/a.png", false),
        ("http://127.0.0.1:8080/a.png", false),
        ("http://[::1]/a.png", false),
        ("http://169.254.169.254/latest/meta", false),
        ("http://0.0.0.0/a.png", false),
        ("http://localhost./a.png", false),
        ("http://[::ffff:127.0.0.1]/a.png", false),
        ("http://[::ffff:192.168.1.1]/a.png", false),
        ("http://[fc00::1]/a.png", false),
        ("http://[fe80::1%25eth0]/a.png", false),
        ("http://224.0.0.1/a.png", false),
        ("http://[ff02::1]/a.png", false),
        ("https:///a.png", false),
        // Forms a browser or the URL parser reads as something else.
        ("https:example.com/a.png", false),
        ("https:\\\\127.0.0.1/a.png", false),
        ("\\\\example.com\\a.png", false),
        ("http://2130706433/a.png", false),
        ("http://0x7f.1/a.png", false),
        ("http://%31%32%37.0.0.1/a.png", false),
        ("http://example.com@127.0.0.1/a.png", false),
        ("http://user:pw@example.com/a.png", false),
        ("ht\ttp://example.com/a.png", false),
        ("javascript:alert(1)", false),
        ("data:image/png;base64,AA", false),
    ] {
        assert_eq!(parse(raw).is_ok(), ok, "{raw}");
    }
}

#[test]
fn address_is_read_the_way_it_is_requested() {
    assert_eq!(parse("//example.com/a b.png#x").unwrap().as_str(), "https://example.com/a%20b.png");
    assert_eq!(parse("https://bücher.example/a.png").unwrap().host_str(), Some("xn--bcher-kva.example"));
}

#[test]
fn fetch_refuses_what_is_not_a_plain_image() {
    let fixture = server(|req, s| match path_of(req) {
        "/a.png" => respond(s, "200 OK", "Content-Type: image/png; x=y\r\n", b"\x89PNG"),
        "/page" => respond(s, "200 OK", "Content-Type: text/html\r\n", b"<script>alert(1)</script>"),
        "/none" => respond(s, "200 OK", "", b"\x89PNG"),
        "/huge.png" => respond(s, "200 OK", "Content-Type: image/png\r\n", &vec![0u8; MAX_BYTES as usize + 1]),
        "/to-file" => respond(s, "302 Found", "Location: file:///etc/passwd\r\n", b""),
        "/to-local" => respond(s, "302 Found", "Location: http://localhost:1/a.png\r\n", b""),
        "/to-a" => respond(s, "302 Found", "Location: /a.png\r\n", b""),
        "/no-location" => respond(s, "302 Found", "", b""),
        _ => respond(s, "404 Not Found", "", b""),
    });
    let c = client_for(fixture);
    let base = "http://image.example";
    let img = c.fetch(&format!("{base}/a.png"), &flag()).unwrap();
    assert_eq!((img.mime.as_str(), img.data.as_slice()), ("image/png", b"\x89PNG".as_slice()));
    assert_eq!(c.fetch(&format!("{base}/to-a"), &flag()).unwrap().mime, "image/png");
    for (p, want) in [
        ("/page", Error::NotAnImage),
        ("/none", Error::NotAnImage),
        ("/huge.png", Error::TooLarge),
        ("/to-file", Error::Refused),
        ("/to-local", Error::Refused),
        ("/no-location", Error::Status(302)),
        ("/missing.png", Error::Status(404)),
    ] {
        assert_eq!(c.fetch(&format!("{base}{p}"), &flag()).unwrap_err(), want, "{p}");
    }
}

#[test]
fn redirects_are_limited() {
    let hops = Arc::new(AtomicUsize::new(0));
    let seen = hops.clone();
    let fixture = server(move |req, s| {
        seen.fetch_add(1, Ordering::SeqCst);
        let n: usize = path_of(req)[1..].parse().unwrap();
        if n == 0 {
            respond(s, "200 OK", "Content-Type: image/png\r\n", b"PNG");
        } else {
            respond(s, "302 Found", &format!("Location: /{}\r\n", n - 1), b"");
        }
    });
    let c = client_for(fixture);
    assert!(c.fetch(&format!("http://image.example/{MAX_REDIRECTS}"), &flag()).is_ok());
    hops.store(0, Ordering::SeqCst);
    assert_eq!(c.fetch(&format!("http://image.example/{}", MAX_REDIRECTS + 1), &flag()).unwrap_err(), Error::TooManyRedirects);
    assert_eq!(hops.load(Ordering::SeqCst), MAX_REDIRECTS + 1, "the last redirect isn't followed");
}

#[test]
fn nothing_identifying_is_sent() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = requests.clone();
    let fixture = server(move |req, s| {
        log.lock().unwrap().push(req.to_ascii_lowercase());
        respond(s, "200 OK", "Content-Type: image/png\r\nSet-Cookie: id=1; Path=/\r\n", b"PNG");
    });
    let c = client_for(fixture);
    for _ in 0..2 {
        c.fetch("http://image.example/a.png", &flag()).unwrap();
    }
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for r in requests.iter() {
        assert!(!r.contains("cookie:") && !r.contains("referer:") && !r.contains("authorization:"), "{r}");
        assert!(r.contains("accept: image/*"), "{r}");
        assert!(!r.contains("accept-encoding: gzip"), "{r}");
    }
}

#[test]
fn a_stalled_site_runs_out_of_time() {
    let fixture = server(|_, s| {
        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 10\r\n\r\nabc");
        std::thread::sleep(Duration::from_secs(10));
    });
    let c = Client::with_parts(
        Box::new(|_, _, _| Ok(vec![SocketAddr::new(PUBLIC.parse().unwrap(), 80)])),
        Some(Box::new(move |_| Ok(fixture))),
        ureq::tls::TlsConfig::default(),
        Duration::from_millis(500),
    );
    let t = Instant::now();
    assert!(c.fetch("http://image.example/a.png", &flag()).is_err());
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
}

/// `TestServeCancelsFetch`: closing a document stops its fetches, even mid-read.
#[test]
fn cancel_stops_a_stalled_read() {
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (closed_tx, closed_rx) = std::sync::mpsc::channel();
    let fixture = server(move |_, s| {
        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 10\r\n\r\nabc");
        let _ = started_tx.send(());
        // The read ends when the other side closes.
        let mut sink = [0u8; 16];
        let _ = s.read(&mut sink);
        let _ = closed_tx.send(());
    });
    let c = client_for(fixture);
    let cancel = flag();
    let flag2 = cancel.clone();
    let done = std::thread::spawn(move || c.fetch("http://image.example/slow.png", &flag2));
    started_rx.recv_timeout(Duration::from_secs(3)).expect("the fetch started");
    cancel.store(true, Ordering::Relaxed);
    let t = Instant::now();
    while !done.is_finished() {
        assert!(t.elapsed() < Duration::from_secs(1), "the fetch survived being cancelled");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(done.join().unwrap().unwrap_err(), Error::Cancelled);
    closed_rx.recv_timeout(Duration::from_secs(1)).expect("the connection stayed open");
}

#[test]
fn unsafe_answers_are_never_dialled() {
    for list in [
        &["127.0.0.1"][..],
        &["10.0.0.1"],
        &["172.16.0.1"],
        &["192.168.0.1"],
        &["169.254.169.254"],
        &["0.0.0.0"],
        &["224.0.0.1"],
        &["::"],
        &["::1"],
        &["fc00::1"],
        &["fe80::1"],
        &["ff02::1"],
        &["::ffff:10.0.0.1"],
        &["93.184.216.34", "127.0.0.1"],
        &[],
        &["100.100.100.200"],
        &["100.64.0.1"],
        &["100.127.255.254"],
        &["0.0.0.1"],
        &["198.18.0.1"],
        &["240.0.0.1"],
        &["::ffff:100.100.100.200"],
        &["64:ff9b::a00:1"],
        &["64:ff9b:1::a00:1"],
        &["2002:a00:1::1"],
        &["fec0::1"],
        &["::127.0.0.1"],
        &["::ffff:0:127.0.0.1"],
        &["2001:2::1"],
        &["4000::1"],
        &["93.184.216.34", "fec0::1"],
    ] {
        let dialled = Arc::new(AtomicUsize::new(0));
        let seen = dialled.clone();
        let answers = ips(list);
        let c = Client::with_parts(
            Box::new(move |_, _, _| Ok(answers.iter().map(|ip| SocketAddr::new(*ip, 80)).collect())),
            Some(Box::new(move |_| {
                seen.fetch_add(1, Ordering::SeqCst);
                Err(std::io::Error::other("unexpected dial"))
            })),
            ureq::tls::TlsConfig::default(),
            Duration::from_secs(5),
        );
        assert!(c.fetch("http://image.example/a.png", &flag()).is_err(), "{list:?}");
        assert_eq!(dialled.load(Ordering::SeqCst), 0, "{list:?}");
    }
}

#[test]
fn public_ip_table() {
    for raw in ips(&["8.8.8.8", "93.184.216.34", "100.63.255.255", "100.128.0.1", "::ffff:8.8.8.8", "2606:4700:4700::1111"]) {
        assert!(public_ip(raw), "public destination refused: {raw}");
    }
    for raw in ips(&[
        "100.64.0.1",
        "100.100.100.200",
        "0.1.2.3",
        "198.19.255.254",
        "240.0.0.1",
        "255.255.255.255",
        "192.0.2.1",
        "198.51.100.1",
        "203.0.113.1",
        "2001:db8::1",
        "2001::1",
        "64:ff9b::7f00:1",
        "64:ff9b:1::1",
        "2002:7f00:1::1",
        "fec0::1",
        "::127.0.0.1",
        "::ffff:0:127.0.0.1",
        "2001:2::1",
        "4000::1",
    ]) {
        assert!(!public_ip(raw), "special-purpose destination allowed: {raw}");
        let u = match raw {
            IpAddr::V4(v) => format!("http://{v}:80/a.png"),
            IpAddr::V6(v) => format!("http://[{v}]:80/a.png"),
        };
        assert!(parse(&u).is_err(), "literal destination allowed: {u}");
    }
}

/// `TestDialPinsResolvedIP`: one lookup, the address it gave, and no proxy from the environment.
#[test]
fn the_connection_uses_the_answer_that_was_checked() {
    // Set for this process only; nothing else in these tests reads them.
    std::env::set_var("HTTP_PROXY", "http://127.0.0.1:1");
    std::env::set_var("HTTPS_PROXY", "http://127.0.0.1:1");
    std::env::set_var("ALL_PROXY", "http://127.0.0.1:1");
    let lookups = Arc::new(AtomicUsize::new(0));
    let seen = lookups.clone();
    let dialled = Arc::new(Mutex::new(Vec::new()));
    let log = dialled.clone();
    let c = Client::with_parts(
        Box::new(move |_, _, _| {
            let ip = if seen.fetch_add(1, Ordering::SeqCst) == 0 { PUBLIC } else { "127.0.0.1" };
            Ok(vec![SocketAddr::new(ip.parse().unwrap(), 80)])
        }),
        Some(Box::new(move |a| {
            log.lock().unwrap().push(a);
            Err(std::io::Error::other("controlled dial failure"))
        })),
        ureq::tls::TlsConfig::default(),
        Duration::from_secs(5),
    );
    assert!(c.agent.config().proxy().is_none(), "proxy enabled");
    let _ = c.fetch("http://image.example/a.png", &flag());
    assert_eq!(lookups.load(Ordering::SeqCst), 1);
    assert_eq!(dialled.lock().unwrap().as_slice(), &[format!("{PUBLIC}:80").parse().unwrap()]);
}

/// The real resolver, with a real name for this machine and a real server listening on it. The
/// address form isn't refused here, because it goes straight to the agent; only the check made at
/// connection time stands between it and the server.
#[test]
fn the_real_client_will_not_connect_to_this_machine() {
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = hits.clone();
    let fixture = server(move |_, s| {
        seen.fetch_add(1, Ordering::SeqCst);
        respond(s, "200 OK", "Content-Type: image/png\r\n", b"PNG");
    });
    let c = Client::new();
    for url in [format!("http://localhost:{}/a.png", fixture.port()), format!("http://{fixture}/a.png")] {
        assert!(c.agent.get(&url).call().is_err(), "{url}");
    }
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[test]
fn redirects_are_checked_at_connection_time() {
    for dest in ["private.example", "192.168.1.1"] {
        let fixture = server(move |_, s| respond(s, "302 Found", &format!("Location: http://{dest}/a.png\r\n"), b""));
        let dials = Arc::new(AtomicUsize::new(0));
        let seen = dials.clone();
        let c = Client::with_parts(
            Box::new(|uri, _, _| {
                let ip = if uri.host() == Some("private.example") { "10.0.0.1" } else { PUBLIC };
                Ok(vec![SocketAddr::new(ip.parse().unwrap(), 80)])
            }),
            Some(Box::new(move |_| {
                seen.fetch_add(1, Ordering::SeqCst);
                Ok(fixture)
            })),
            ureq::tls::TlsConfig::default(),
            Duration::from_secs(5),
        );
        assert!(c.fetch("http://image.example/start", &flag()).is_err(), "{dest}");
        assert_eq!(dials.load(Ordering::SeqCst), 1, "{dest}");
    }
}

/// `TestTLSVerification`: the certificate is checked against the name asked for, not the address
/// dialled.
#[test]
fn tls_is_verified_against_the_host_name() {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};

    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_cert = ca.self_signed(&ca_key).unwrap();
    let issuer = rcgen::Issuer::new(ca, ca_key);
    let leaf_key = rcgen::KeyPair::generate().unwrap();
    let leaf = rcgen::CertificateParams::new(vec!["example.com".to_string()]).unwrap().signed_by(&leaf_key, &issuer).unwrap();

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let chain = vec![CertificateDer::from(leaf.der().to_vec())];
    let key = PrivateKeyDer::try_from(leaf_key.serialize_der()).unwrap();
    let server_config = Arc::new(
        rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let fixture = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            let config = server_config.clone();
            std::thread::spawn(move || {
                let Ok(conn) = rustls::ServerConnection::new(config) else { return };
                let mut tls = rustls::StreamOwned::new(conn, s);
                let _ = read_request(&mut tls);
                respond(&mut tls, "200 OK", "Content-Type: image/png\r\n", b"PNG");
                tls.conn.send_close_notify();
                let _ = tls.flush();
            });
        }
    });

    let roots = ureq::tls::RootCerts::from(vec![ureq::tls::Certificate::from_der(ca_cert.der()).to_owned()]);
    let tls = ureq::tls::TlsConfig::builder().root_certs(roots).build();
    let c = Client::with_parts(
        Box::new(|_, _, _| Ok(vec![SocketAddr::new(PUBLIC.parse().unwrap(), 443)])),
        Some(Box::new(move |_| Ok(fixture))),
        tls,
        Duration::from_secs(5),
    );
    assert_eq!(c.fetch("https://example.com/a.png", &flag()).unwrap().data, b"PNG");
    assert!(c.fetch("https://wrong.example/a.png", &flag()).is_err(), "accepted the wrong host name");

    // And the built-in roots don't trust the fixture's authority.
    let strict = Client::with_parts(
        Box::new(|_, _, _| Ok(vec![SocketAddr::new(PUBLIC.parse().unwrap(), 443)])),
        Some(Box::new(move |_| Ok(fixture))),
        ureq::tls::TlsConfig::default(),
        Duration::from_secs(5),
    );
    assert!(strict.fetch("https://example.com/a.png", &flag()).is_err(), "accepted an unknown authority");
}

#[test]
fn cache_expires_and_stays_within_its_budget() {
    let image = |n: usize| Arc::new(Image { mime: "image/png".into(), data: vec![0; n] });
    let t0 = Instant::now();
    let mut c = Cache::new(100, Duration::from_secs(10));
    c.insert("a", image(60), t0);
    assert!(c.get("a", t0 + Duration::from_secs(9)).is_some());
    assert!(c.get("a", t0 + Duration::from_secs(10)).is_none(), "expired");
    assert_eq!(c.bytes, 0);

    c.insert("a", image(60), t0);
    c.insert("b", image(30), t0 + Duration::from_secs(1));
    c.insert("c", image(30), t0 + Duration::from_secs(2));
    assert!(c.get("a", t0 + Duration::from_secs(3)).is_none(), "the oldest went first");
    assert_eq!(c.bytes, 60);
    c.insert("d", image(101), t0 + Duration::from_secs(3));
    assert!(c.get("d", t0 + Duration::from_secs(3)).is_none(), "too big to keep");
    c.insert("b", image(10), t0 + Duration::from_secs(4));
    assert_eq!(c.bytes, 40, "replacing an entry gives its bytes back");
}
