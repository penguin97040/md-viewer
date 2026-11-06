//! Fetches images from the web for a document whose reader has agreed to load them. Nothing in the
//! app contacts the internet except through here, and nothing calls here before that agreement.
//!
//! Addresses are checked twice: as written, including every redirect, and again by the resolver at
//! the moment of connecting, on every address a name resolves to. The connection can only go to an
//! address that passed, so a name that changes its answer (DNS rebinding) or a redirect to a private
//! address cannot reach a machine on the reader's own network. Environment proxies are not used,
//! because they would resolve and connect outside these checks.

use std::collections::HashMap;
use std::fmt;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{Buffers, ConnectionDetails, Connector, Either, NextTimeout, RustlsConnector, TcpConnector, Transport};
use ureq::Agent;
use url::{Host, Url};

/// The largest image fetched.
pub const MAX_BYTES: u64 = 25 << 20;
/// Redirects followed for one image.
pub const MAX_REDIRECTS: usize = 5;
/// One image, start to finish, including its redirects.
const FETCH_TIME: Duration = Duration::from_secs(30);
const CONNECT_TIME: Duration = Duration::from_secs(10);
/// How long a cancelled fetch can sit in a read before it notices.
const CANCEL_POLL: Duration = Duration::from_millis(200);
/// How long a fetched image is reused: `Cache-Control: private, max-age=3600` in the Go reader.
/// Opening a document again within the hour doesn't tell its sites again.
const CACHE_TIME: Duration = Duration::from_secs(3600);
/// Fetched bytes kept in memory. Nothing is written to disk.
const CACHE_BYTES: u64 = 128 << 20;

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// Not a web address, or one that points at a private or special-purpose address.
    Refused,
    TooManyRedirects,
    Status(u16),
    NotAnImage,
    TooLarge,
    Cancelled,
    Network(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Refused => f.write_str("not a public web address"),
            Error::TooManyRedirects => f.write_str("too many redirects"),
            Error::Status(s) => write!(f, "the site answered {s}"),
            Error::NotAnImage => f.write_str("not an image"),
            Error::TooLarge => f.write_str("image too large"),
            Error::Cancelled => f.write_str("cancelled"),
            Error::Network(m) => f.write_str(m),
        }
    }
}

/// A fetched image: its lower-case media type and its bytes.
#[derive(Debug)]
pub struct Image {
    pub mime: String,
    pub data: Vec<u8>,
}

/// Fetches an image, or returns the copy fetched within the last hour. Runs until the image is
/// done, the time limit passes or `cancel` is set. Anything that isn't an image is refused.
pub fn fetch(raw: &str, cancel: &Arc<AtomicBool>) -> Result<Arc<Image>, Error> {
    static CLIENT: LazyLock<Client> = LazyLock::new(Client::new);
    if let Some(hit) = cached(raw) {
        return Ok(hit);
    }
    let image = Arc::new(CLIENT.fetch(raw, cancel)?);
    locked(&CACHE).insert(raw, image.clone(), Instant::now());
    Ok(image)
}

/// The copy of an image fetched within the last hour, without contacting anything.
pub fn cached(raw: &str) -> Option<Arc<Image>> {
    locked(&CACHE).get(raw, Instant::now())
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(Cache::new(CACHE_BYTES, CACHE_TIME)));

/// A panic elsewhere must not take image loading down with it, and the cache stays consistent
/// because every change to it is made in one step.
fn locked(m: &Mutex<Cache>) -> MutexGuard<'_, Cache> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct Cached {
    at: Instant,
    image: Arc<Image>,
}

struct Cache {
    entries: HashMap<String, Cached>,
    bytes: u64,
    budget: u64,
    keep: Duration,
}

impl Cache {
    fn new(budget: u64, keep: Duration) -> Self {
        Self { entries: HashMap::new(), bytes: 0, budget, keep }
    }

    fn get(&mut self, key: &str, now: Instant) -> Option<Arc<Image>> {
        let e = self.entries.get(key)?;
        if now.duration_since(e.at) < self.keep {
            return Some(e.image.clone());
        }
        self.remove(key);
        None
    }

    fn remove(&mut self, key: &str) {
        if let Some(e) = self.entries.remove(key) {
            self.bytes -= e.image.data.len() as u64;
        }
    }

    /// Stores an image, dropping the oldest entries to stay within the budget.
    fn insert(&mut self, key: &str, image: Arc<Image>, now: Instant) {
        let size = image.data.len() as u64;
        self.remove(key);
        if size > self.budget {
            return;
        }
        while self.bytes + size > self.budget {
            let Some(oldest) = self.entries.iter().min_by_key(|(_, e)| e.at).map(|(k, _)| k.clone()) else { break };
            self.remove(&oldest);
        }
        self.bytes += size;
        self.entries.insert(key.to_string(), Cached { at: now, image });
    }
}

// ---- which addresses are public -----------------------------------------------------------------

/// IPv4 ranges that are not public destinations. Multicast and the broadcast address are included.
const V4_BLOCKED: &[(u32, u32)] = &[
    (0x0000_0000, 8),  // 0.0.0.0/8
    (0x0a00_0000, 8),  // 10.0.0.0/8
    (0x6440_0000, 10), // 100.64.0.0/10, shared address space, which includes cloud metadata services
    (0x7f00_0000, 8),  // 127.0.0.0/8
    (0xa9fe_0000, 16), // 169.254.0.0/16
    (0xac10_0000, 12), // 172.16.0.0/12
    (0xc000_0000, 24), // 192.0.0.0/24
    (0xc000_0200, 24), // 192.0.2.0/24
    (0xc0a8_0000, 16), // 192.168.0.0/16
    (0xc612_0000, 15), // 198.18.0.0/15
    (0xc633_6400, 24), // 198.51.100.0/24
    (0xcb00_7100, 24), // 203.0.113.0/24
    (0xe000_0000, 4),  // 224.0.0.0/4
    (0xf000_0000, 4),  // 240.0.0.0/4
];

/// Special-purpose ranges inside global unicast. Translation and tunnel prefixes can route an
/// embedded IPv4 address to an internal service even when the IPv6 address looks public.
const V6_BLOCKED: &[(u128, u32)] = &[
    (0x0100_0000_0000_0000 << 64, 64),                  // 100::/64
    (0x2001_0000u128 << 96, 32),                        // 2001::/32
    (0x2001_0002u128 << 96, 48),                        // 2001:2::/48
    (0x2001_0010u128 << 96, 28),                        // 2001:10::/28
    (0x2001_0020u128 << 96, 28),                        // 2001:20::/28
    (0x2001_0db8u128 << 96, 32),                        // 2001:db8::/32
    (0x2002u128 << 112, 16),                            // 2002::/16
    (0x3fffu128 << 112, 20),                            // 3fff::/20
    (0x5f00u128 << 112, 16),                            // 5f00::/16
];

fn in_prefix_v4(ip: u32, net: u32, bits: u32) -> bool {
    ip >> (32 - bits) == net >> (32 - bits)
}

fn in_prefix_v6(ip: u128, net: u128, bits: u32) -> bool {
    ip >> (128 - bits) == net >> (128 - bits)
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let ip = u32::from(ip);
    !V4_BLOCKED.iter().any(|&(net, bits)| in_prefix_v4(ip, net, bits))
}

fn public_v6(ip: Ipv6Addr) -> bool {
    let ip = u128::from(ip);
    // Only the allocated global-unicast space is eligible. This also rules out loopback,
    // link-local, site-local, unique-local, multicast, IPv4-compatible and translation addresses.
    in_prefix_v6(ip, 0x2000u128 << 112, 3) && !V6_BLOCKED.iter().any(|&(net, bits)| in_prefix_v6(ip, net, bits))
}

/// Whether an address is a public destination.
pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => public_v4(v4),
            None => public_v6(v6),
        },
    }
}

/// Parses an address the way it will be requested, refusing unsafe forms and literal addresses.
/// DNS answers are checked again at connection time, including on redirects.
fn parse(raw: &str) -> Result<Url, Error> {
    let raw = raw.trim();
    if raw.chars().any(|c| c.is_control()) {
        return Err(Error::Refused);
    }
    let owned;
    let raw = if raw.starts_with("//") {
        owned = format!("https:{raw}");
        owned.as_str()
    } else {
        raw
    };
    // The URL parser would read `https:host/x` and `https:///host/x` as `https://host/x`, as a
    // browser does. The Go reader refused them, and nothing legitimate writes them.
    let rest = raw.split_once(':').map_or("", |(_, r)| r);
    if !rest.starts_with("//") || rest[2..].starts_with(['/', '\\']) {
        return Err(Error::Refused);
    }
    check(Url::parse(raw).map_err(|_| Error::Refused)?)
}

fn check(mut u: Url) -> Result<Url, Error> {
    if !matches!(u.scheme(), "http" | "https") {
        return Err(Error::Refused);
    }
    // Credentials in an address go nowhere useful here, and `http://public@127.0.0.1/` is a classic
    // way of making an address read as something it isn't.
    if !u.username().is_empty() || u.password().is_some() {
        return Err(Error::Refused);
    }
    match u.host() {
        Some(Host::Domain(d)) => {
            let host = d.trim_end_matches('.').to_ascii_lowercase();
            if host.is_empty() || host == "localhost" || host.ends_with(".localhost") || host.contains('%') {
                return Err(Error::Refused);
            }
        }
        Some(Host::Ipv4(ip)) if public_ip(IpAddr::V4(ip)) => {}
        Some(Host::Ipv6(ip)) if public_ip(IpAddr::V6(ip)) => {}
        _ => return Err(Error::Refused),
    }
    u.set_fragment(None);
    Ok(u)
}

// ---- the connection ------------------------------------------------------------------------------

/// `ResolvedSocketAddrs` holds this many.
const MAX_ADDRS: usize = 16;

type Lookup = dyn Fn(&Uri, &Config, NextTimeout) -> Result<Vec<SocketAddr>, ureq::Error> + Send + Sync;

/// Validates what a name resolves to at the moment of connecting. The transport connects only to
/// what this returns and doesn't look the name up again, so a second answer can't change it.
struct CheckedResolver {
    lookup: Box<Lookup>,
}

impl fmt::Debug for CheckedResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CheckedResolver")
    }
}

impl Resolver for CheckedResolver {
    fn resolve(&self, uri: &Uri, config: &Config, timeout: NextTimeout) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let found = (self.lookup)(uri, config, timeout)?;
        // Every answer must be public, not only the one that happens to be tried first.
        if found.is_empty() || found.iter().any(|a| !public_ip(a.ip())) {
            return Err(ureq::Error::HostNotFound);
        }
        let mut out = self.empty();
        for a in found.into_iter().take(MAX_ADDRS) {
            out.push(a);
        }
        Ok(out)
    }
}

thread_local! {
    /// The running fetch's cancel flag. A fetch runs on one thread from start to finish.
    static CANCEL: std::cell::RefCell<Option<Arc<AtomicBool>>> = const { std::cell::RefCell::new(None) };
}

fn cancelled() -> bool {
    CANCEL.with(|c| c.borrow().as_ref().is_some_and(|f| f.load(Ordering::Relaxed)))
}

/// Breaks long reads into short ones so a fetch nobody wants any more stops within `CANCEL_POLL`.
#[derive(Debug)]
struct CancelTransport<T>(T);

impl<T: Transport> Transport for CancelTransport<T> {
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.0.buffers()
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        self.0.transmit_output(amount, timeout)
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        let total = timeout.not_zero().map_or(Duration::ZERO, |d| *d);
        let started = Instant::now();
        loop {
            if cancelled() {
                return Err(ureq::Error::Io(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled")));
            }
            let left = total.saturating_sub(started.elapsed());
            let slice = NextTimeout { after: left.min(CANCEL_POLL).into(), ..timeout };
            match self.0.await_input(slice) {
                Err(ureq::Error::Timeout(_)) if started.elapsed() < total => {}
                other => return other,
            }
        }
    }

    fn is_open(&mut self) -> bool {
        self.0.is_open()
    }

    fn is_tls(&self) -> bool {
        self.0.is_tls()
    }
}

/// Opens the socket. `route` only exists for the tests, which see a public address and connect to a
/// local fixture; there's no such exception in a real connection, where it's the identity.
type Route = dyn Fn(SocketAddr) -> std::io::Result<SocketAddr> + Send + Sync;

struct Dial {
    tcp: TcpConnector,
    route: Option<Box<Route>>,
}

impl fmt::Debug for Dial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Dial")
    }
}

impl<In: Transport> Connector<In> for Dial {
    type Out = Either<In, CancelTransport<<TcpConnector as Connector<In>>::Out>>;

    fn connect(&self, details: &ConnectionDetails, chained: Option<In>) -> Result<Option<Self::Out>, ureq::Error> {
        if let Some(existing) = chained {
            return Ok(Some(Either::A(existing)));
        }
        let wrap = |t: Option<_>| t.map(|t| Either::B(CancelTransport(t)));
        let Some(route) = &self.route else {
            return self.tcp.connect(details, None).map(wrap);
        };
        let mut addrs = details.addrs.clone();
        for a in addrs.iter_mut() {
            *a = route(*a)?;
        }
        let routed = ConnectionDetails {
            uri: details.uri,
            addrs,
            config: details.config,
            request_level: details.request_level,
            resolver: details.resolver,
            now: details.now,
            timeout: details.timeout,
            current_time: details.current_time.clone(),
            run_connector: details.run_connector.clone(),
            register_shutdown: details.register_shutdown.clone(),
        };
        self.tcp.connect(&routed, None).map(wrap)
    }
}

struct Client {
    agent: Agent,
    limit: Duration,
}

impl Client {
    fn new() -> Self {
        Self::with_parts(
            Box::new(|uri, config, timeout| Ok(DefaultResolver::default().resolve(uri, config, timeout)?.iter().copied().collect())),
            None,
            ureq::tls::TlsConfig::default(),
            FETCH_TIME,
        )
    }

    fn with_parts(lookup: Box<Lookup>, route: Option<Box<Route>>, tls: ureq::tls::TlsConfig, limit: Duration) -> Self {
        let config = Agent::config_builder()
            // Proxies would resolve and connect outside the checks above.
            .proxy(None)
            // Redirects are followed by hand so that each is checked before anything happens.
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_connect(Some(CONNECT_TIME))
            .tls_config(tls)
            .user_agent("MD-Viewer")
            .accept("image/*")
            .build();
        let connector = ().chain(Dial { tcp: TcpConnector::default(), route }).chain(RustlsConnector::default());
        Self { agent: Agent::with_parts(config, connector, CheckedResolver { lookup }), limit }
    }

    fn fetch(&self, raw: &str, cancel: &Arc<AtomicBool>) -> Result<Image, Error> {
        CANCEL.with(|c| *c.borrow_mut() = Some(cancel.clone()));
        let r = self.fetch_inner(raw, cancel);
        CANCEL.with(|c| *c.borrow_mut() = None);
        r
    }

    fn fetch_inner(&self, raw: &str, cancel: &AtomicBool) -> Result<Image, Error> {
        let started = Instant::now();
        let mut url = parse(raw)?;
        let mut redirects = 0;
        loop {
            let left = self.limit.checked_sub(started.elapsed()).ok_or_else(|| Error::Network("took too long".into()))?;
            let mut resp = self.agent.get(url.as_str()).config().timeout_global(Some(left)).build().call().map_err(|e| failure(e, cancel))?;
            let status = resp.status().as_u16();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                redirects += 1;
                if redirects > MAX_REDIRECTS {
                    return Err(Error::TooManyRedirects);
                }
                let to = resp.headers().get("location").and_then(|v| v.to_str().ok()).ok_or(Error::Status(status))?;
                url = check(url.join(to).map_err(|_| Error::Refused)?)?;
                continue;
            }
            if status != 200 {
                return Err(Error::Status(status));
            }
            let mime = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
            let mime = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
            if !mime.starts_with("image/") {
                return Err(Error::NotAnImage);
            }
            let mut data = Vec::new();
            let mut reader = resp.body_mut().with_config().limit(MAX_BYTES + 1).reader();
            // Short reads, so a cancel can't wait behind a large body arriving steadily.
            let mut buf = [0u8; 16 * 1024];
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                let n = reader.read(&mut buf).map_err(|e| read_failure(&e, cancel))?;
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
                if data.len() as u64 > MAX_BYTES {
                    return Err(Error::TooLarge);
                }
            }
            return Ok(Image { mime, data });
        }
    }
}

fn failure(e: ureq::Error, cancel: &AtomicBool) -> Error {
    if cancel.load(Ordering::Relaxed) {
        Error::Cancelled
    } else {
        Error::Network(e.to_string())
    }
}

fn read_failure(e: &std::io::Error, cancel: &AtomicBool) -> Error {
    if cancel.load(Ordering::Relaxed) {
        Error::Cancelled
    } else if e.to_string().contains("limit") {
        Error::TooLarge
    } else {
        Error::Network(e.to_string())
    }
}

#[cfg(test)]
mod tests;
