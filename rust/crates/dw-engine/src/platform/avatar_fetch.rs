//! The avatar fetch guard (DASHPAY §3.8; DP4-02 design, "The fetch
//! guard"). Avatar URLs come from other users' Platform documents, so every
//! hop is checked before anything connects to it: `https` on the default
//! port without user info, and every address the name resolves to public.
//! The HTTP client gets only the addresses that were checked (its resolver
//! is [`Checked`]), so it never resolves a name itself and a rebinding DNS
//! server cannot move a connection. Redirects are followed here, through
//! the same checks; the body is capped in size and the whole fetch in time.
//! Not part of the facade.

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::{ACCEPT, CONTENT_LENGTH, LOCATION};
use reqwest::redirect::Policy as RedirectPolicy;
use reqwest::{StatusCode, Url};

/// The largest avatar fetched or accepted from a file (§3.8).
pub(super) const MAX_BYTES: usize = 5 << 20;
/// The longest avatar URL (the contract's `maxLength`; the library drops
/// longer ones).
pub(super) const MAX_URL_LEN: usize = 2048;
/// The whole fetch, every hop included (§3.8).
const FETCH_TIME: Duration = Duration::from_secs(10);
const CONNECT_TIME: Duration = Duration::from_secs(5);
/// Redirects followed after the first request.
const MAX_REDIRECTS: usize = 3;

/// Why a fetch failed. All but `TooLarge` are `avatar.fetch_failed` to a
/// host; the variant is kept for logs and tests.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum FetchError {
    /// The URL breaks a rule of its form (scheme, port, user info, length).
    #[error("refused URL: {0}")]
    Form(String),
    /// A host's address is not public (a literal, or a name's answer).
    #[error("refused address: {0}")]
    Address(String),
    #[error("over {MAX_BYTES} bytes")]
    TooLarge,
    #[error("HTTP status {0}")]
    Status(u16),
    #[error("more than {MAX_REDIRECTS} redirects")]
    Redirects,
    #[error("timed out")]
    Timeout,
    #[error("{0}")]
    Network(String),
}

/// What counts as public, and which URLs are fetched. Production uses
/// [`FetchPolicy::STRICT`]; tests may also reach a local server (D20).
#[derive(Debug, Clone)]
pub(super) struct FetchPolicy {
    /// `https` on port 443 only (production).
    pub(super) https_only: bool,
    /// Addresses treated as public although they are not (tests only).
    pub(super) also_public: Vec<IpAddr>,
}

impl FetchPolicy {
    pub(super) const STRICT: Self = Self {
        https_only: true,
        also_public: Vec::new(),
    };

    /// The URL rules that need no network: scheme, port, user info and
    /// length, and an IP-literal host's address.
    pub(super) fn check_url(&self, url: &Url) -> Result<(), FetchError> {
        let form = |why: &str| Err(FetchError::Form(why.into()));
        if url.as_str().len() > MAX_URL_LEN {
            return form("longer than 2048 bytes");
        }
        match (url.scheme(), self.https_only) {
            ("https", _) | ("http", false) => {}
            _ => return form("not https"),
        }
        if !url.username().is_empty() || url.password().is_some() {
            return form("carries a user name or password");
        }
        if self.https_only && url.port().is_some_and(|p| p != 443) {
            return form("not the default port");
        }
        match url.host() {
            None => form("no host"),
            Some(url::Host::Domain(_)) => Ok(()),
            Some(url::Host::Ipv4(ip)) => self.check_ip(IpAddr::V4(ip)),
            Some(url::Host::Ipv6(ip)) => self.check_ip(IpAddr::V6(ip)),
        }
    }

    fn check_ip(&self, ip: IpAddr) -> Result<(), FetchError> {
        if is_public(ip) || self.also_public.contains(&ip) {
            Ok(())
        } else {
            Err(FetchError::Address(ip.to_string()))
        }
    }
}

/// Whether `ip` is a public unicast address (design: "Addresses").
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0
        || a == 10
        || (a == 100 && (64..128).contains(&b))
        || a == 127
        || (a == 169 && b == 254)
        || (a == 172 && (16..32).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 88 && c == 99)
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    // Forms that carry an IPv4 address: judge that address.
    let embedded = |hi: u16, lo: u16| Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo));
    if s[..5] == [0; 5] && (s[5] == 0xffff || s[5] == 0) {
        // ::ffff:a.b.c.d (mapped) and ::a.b.c.d (compatible); `::` and `::1`
        // fall here too and are not public.
        return is_public_v4(embedded(s[6], s[7]));
    }
    if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return is_public_v4(embedded(s[6], s[7]));
    }
    // Global unicast only, without the special ranges inside it.
    (s[0] & 0xe000) == 0x2000
        && (s[0] != 0x2001 || s[1] >= 0x0200)
        && !(s[0] == 0x2001 && s[1] == 0x0db8)
        && s[0] != 0x2002
        // 3fff::/20, documentation (RFC 9637).
        && !(s[0] == 0x3fff && s[1] < 0x1000)
}

/// Resolves a host name to addresses. The system resolver in production; a
/// script in tests.
pub(super) trait NameResolver: Send + Sync + 'static {
    fn lookup(
        &self,
        host: String,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<IpAddr>>> + Send>>;
}

/// The operating system's resolver (`getaddrinfo` on a blocking thread).
/// A lookup given up on before a blocking thread takes it is not made
/// (DP4-02 D36); one already running finishes, its answer unused.
pub(super) struct SystemResolver;

impl NameResolver for SystemResolver {
    fn lookup(
        &self,
        host: String,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<IpAddr>>> + Send>> {
        Box::pin(async move {
            use std::net::ToSocketAddrs;
            // Dropped with this future, which is when the lookup is given up.
            let wanted = Arc::new(());
            let still = Arc::downgrade(&wanted);
            let lookup = tokio::task::spawn_blocking(move || {
                let Some(_wanted) = still.upgrade() else {
                    return Err(std::io::Error::other("lookup given up"));
                };
                let addrs = (host.as_str(), 0).to_socket_addrs()?;
                Ok(addrs.map(|a| a.ip()).collect())
            });
            lookup.await.map_err(std::io::Error::other)?
        })
    }
}

/// The client's resolver: one lookup, and every address must be public
/// (D4), else the connection is refused before it is made.
struct Checked {
    inner: Arc<dyn NameResolver>,
    policy: FetchPolicy,
}

impl Resolve for Checked {
    fn resolve(&self, name: Name) -> Resolving {
        let lookup = self.inner.lookup(name.as_str().to_owned());
        let policy = self.policy.clone();
        Box::pin(async move {
            let ips = lookup.await?;
            if ips.is_empty() {
                return Err("no address".into());
            }
            for ip in &ips {
                policy.check_ip(*ip)?;
            }
            // Port 0: the client puts the URL's port in.
            let addrs: Addrs = Box::new(ips.into_iter().map(|ip| SocketAddr::new(ip, 0)));
            Ok(addrs)
        })
    }
}

/// The guarded fetcher a session keeps.
pub(super) struct Fetcher {
    /// `None` when the guarded client could not be built (no TLS backend):
    /// every fetch then fails. Never an unconfigured client.
    client: Option<reqwest::Client>,
    policy: FetchPolicy,
}

impl Fetcher {
    pub(super) fn new(policy: FetchPolicy, resolver: Arc<dyn NameResolver>) -> Self {
        let client = reqwest::Client::builder()
            .redirect(RedirectPolicy::none())
            // The environment's proxy variables must not route around the
            // address check (D5).
            .no_proxy()
            .dns_resolver(Arc::new(Checked {
                inner: resolver,
                policy: policy.clone(),
            }))
            .https_only(policy.https_only)
            .connect_timeout(CONNECT_TIME)
            .timeout(FETCH_TIME)
            .referer(false)
            .user_agent("DashWallet-Desktop")
            .build()
            .inspect_err(|e| tracing::warn!(error = %e, "avatar HTTP client unavailable"))
            .ok();
        Self { client, policy }
    }

    /// The URL rules that need no network (the policy's).
    pub(super) fn check_url(&self, url: &Url) -> Result<(), FetchError> {
        self.policy.check_url(url)
    }

    /// The bytes at `url`, following at most [`MAX_REDIRECTS`] redirects,
    /// every hop checked first, within [`FETCH_TIME`] and [`MAX_BYTES`].
    pub(super) async fn fetch(&self, url: &Url) -> Result<Vec<u8>, FetchError> {
        tokio::time::timeout(FETCH_TIME, self.follow(url.clone()))
            .await
            .unwrap_or(Err(FetchError::Timeout))
    }

    async fn follow(&self, mut url: Url) -> Result<Vec<u8>, FetchError> {
        let client = (self.client.as_ref())
            .ok_or_else(|| FetchError::Network("the HTTP client is unavailable".into()))?;
        for _ in 0..=MAX_REDIRECTS {
            self.policy.check_url(&url)?;
            let response = (client.get(url.clone()))
                .header(ACCEPT, "image/png, image/jpeg, image/gif")
                .send()
                .await
                .map_err(network)?;
            let status = response.status();
            if status == StatusCode::OK {
                return body(response).await;
            }
            if !status.is_redirection() {
                return Err(FetchError::Status(status.as_u16()));
            }
            let location = (response.headers().get(LOCATION))
                .and_then(|l| l.to_str().ok())
                .ok_or(FetchError::Status(status.as_u16()))?;
            url = url
                .join(location)
                .map_err(|e| FetchError::Form(format!("redirect: {e}")))?;
        }
        Err(FetchError::Redirects)
    }
}

/// The body, refused past [`MAX_BYTES`] (by its length first, then as read).
async fn body(mut response: reqwest::Response) -> Result<Vec<u8>, FetchError> {
    let declared =
        (response.headers().get(CONTENT_LENGTH)).and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
    if declared.is_some_and(|n| n > MAX_BYTES as u64) {
        return Err(FetchError::TooLarge);
    }
    let mut out = Vec::with_capacity(declared.map_or(64 << 10, |n| n as usize));
    while let Some(chunk) = response.chunk().await.map_err(network)? {
        if out.len() + chunk.len() > MAX_BYTES {
            return Err(FetchError::TooLarge);
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// A client error, with the guard's refusal recovered from its sources.
fn network(e: reqwest::Error) -> FetchError {
    if e.is_timeout() {
        return FetchError::Timeout;
    }
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&e);
    while let Some(s) = source {
        if let Some(refused) = s.downcast_ref::<FetchError>() {
            return refused.clone();
        }
        source = s.source();
    }
    FetchError::Network(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public(text: &str) -> bool {
        is_public(text.parse().unwrap())
    }

    #[test]
    fn private_and_special_addresses_are_not_public() {
        for ip in [
            "0.0.0.0",
            "10.0.0.1",
            "100.64.0.1",
            "100.127.255.255",
            "127.0.0.1",
            "127.255.255.254",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.8",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.7",
            "203.0.113.9",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::ffff:10.0.0.1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "::10.0.0.1",
            "64:ff9b::10.0.0.1",
            "64:ff9b:1::1",
            "100::1",
            "2001::1",
            "2001:10::1",
            "2001:db8::1",
            "2002:a00:1::1",
            "fc00::1",
            "fd00:ec2::254",
            "fe80::1",
            "fec0::1",
            "ff02::1",
            "3fff::1",
            "3fff:fff::1",
        ] {
            assert!(!public(ip), "{ip} counted as public");
        }
    }

    #[test]
    fn global_unicast_is_public() {
        for ip in [
            "1.1.1.1",
            "8.8.8.8",
            "100.63.255.255",
            "100.128.0.1",
            "172.15.255.255",
            "172.32.0.1",
            "192.0.1.1",
            "198.20.0.1",
            "223.255.255.255",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
            "2001:4860:4860::8888",
            "2606:4700:4700::1111",
            "2a00:1450::1",
            "3fff:1000::1",
        ] {
            assert!(public(ip), "{ip} counted as private");
        }
    }

    #[test]
    fn the_url_rules() {
        let p = FetchPolicy::STRICT;
        let check = |u: &str| p.check_url(&Url::parse(u).unwrap());
        assert_eq!(check("https://example.com/a.png"), Ok(()));
        assert_eq!(check("https://example.com:443/a.png"), Ok(()));
        assert!(matches!(
            check("http://example.com/a.png"),
            Err(FetchError::Form(_))
        ));
        assert!(matches!(
            check("ftp://example.com/a.png"),
            Err(FetchError::Form(_))
        ));
        assert!(matches!(
            check("https://example.com:8443/a"),
            Err(FetchError::Form(_))
        ));
        assert!(matches!(
            check("https://u:p@example.com/a"),
            Err(FetchError::Form(_))
        ));
        assert!(matches!(
            check("https://u@example.com/a"),
            Err(FetchError::Form(_))
        ));
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_LEN));
        assert!(matches!(check(&long), Err(FetchError::Form(_))));
        // IP literals, in every spelling the URL parser accepts.
        for u in [
            "https://10.0.0.1/a",
            "https://0x0a.0.0.1/a",
            "https://167772161/a",
            "https://012.0.0.1/a",
            "https://127.1/a",
            "https://[::1]/a",
            "https://[::ffff:a00:1]/a",
            "https://[fd00:ec2::254]/a",
        ] {
            assert!(matches!(check(u), Err(FetchError::Address(_))), "{u}");
        }
        assert_eq!(check("https://8.8.8.8/a"), Ok(()));
    }

    /// What a redirect's `Location` becomes once joined onto an `https` hop:
    /// the production rules refuse each before any request.
    #[test]
    fn redirect_targets_meet_the_same_rules() {
        let p = FetchPolicy::STRICT;
        let base = Url::parse("https://good.example/start").unwrap();
        let hop = |location: &str| p.check_url(&base.join(location).unwrap());
        for (location, form) in [
            ("http://good.example/a.png", true),
            ("https://a@good.example/a.png", true),
            ("https://good.example:444/a.png", true),
            ("//10.0.0.1/x", false),
            ("https://[::ffff:10.0.0.1]/x", false),
            ("https://0x7f000001/x", false),
            ("https://169.254.169.254/latest/meta-data", false),
        ] {
            let refused = hop(location).unwrap_err();
            assert_eq!(
                matches!(refused, FetchError::Form(_)),
                form,
                "{location}: {refused:?}"
            );
        }
        assert_eq!(hop("/next.png"), Ok(()));
    }
}
