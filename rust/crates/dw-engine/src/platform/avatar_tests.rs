//! Tests of the avatar pipeline (DP4-02): a local HTTP server and a
//! scripted resolver behind the production guard, with the test policy of
//! D20 (plain `http`, and `127.0.0.1` counted as public). The acceptance's
//! fixtures: an oversize image, a wrong hash, a redirect to 10.0.0.1 and a
//! GIF; plus DNS rebinding, a private listener that must never see a
//! connection, the cache's descriptor writes, and the decoder's seed corpus.

use std::collections::HashMap;
use std::io::Cursor;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dw_appdb::AppDb;
use image::codecs::gif::GifEncoder;
use image::{DynamicImage, Frame, ImageFormat, Rgba, RgbaImage};
use platform_wallet::wallet::identity::{calculate_avatar_hash, calculate_dhash_fingerprint};
use reqwest::Url;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::avatar::{Admission, Pictures, Pipeline, Target, Whose};
use super::avatar_cache::ThumbDir;
use super::avatar_fetch::{FetchError, FetchPolicy, Fetcher, MAX_BYTES, NameResolver};
use super::avatar_image::{self, Crop, MAX_SIDE};
use super::errors::AvatarError;
use super::profile::{AvatarSize, CropRect, Profile};

pub(super) const LOCAL: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const PRIVATE: IpAddr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2));

// ---- pictures ----

fn solid(w: u32, h: u32, rgba: [u8; 4]) -> RgbaImage {
    RgbaImage::from_pixel(w, h, Rgba(rgba))
}

/// A picture with structure, so its dHash is not all zeros: quadrants of
/// four colours over a gradient.
fn picture(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| {
        let q = [[255u8, 0, 0], [0, 200, 0], [0, 0, 255], [240, 240, 0]]
            [usize::from(x >= w / 2) + 2 * usize::from(y >= h / 2)];
        let g = (((x * 255) / w.max(1)) / 3) as u8;
        Rgba([q[0] / 2 + g, q[1] / 2 + g, q[2] / 2 + g, 255])
    })
}

fn encode(image: &RgbaImage, format: ImageFormat) -> Vec<u8> {
    let image = DynamicImage::ImageRgba8(image.clone());
    let image = match format {
        ImageFormat::Jpeg => DynamicImage::ImageRgb8(image.to_rgb8()),
        _ => image,
    };
    let mut out = Vec::new();
    image.write_to(&mut Cursor::new(&mut out), format).unwrap();
    out
}

pub(super) fn png(w: u32, h: u32) -> Vec<u8> {
    encode(&picture(w, h), ImageFormat::Png)
}

/// An animated GIF: a red frame, then a blue one.
fn animated_gif() -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = GifEncoder::new(&mut out);
        for colour in [[255, 0, 0, 255], [0, 0, 255, 255]] {
            enc.encode_frame(Frame::new(solid(32, 32, colour))).unwrap();
        }
    }
    out
}

/// The profile that publishes `bytes` at `url`, as the library would.
pub(super) fn profile_for(url: &str, bytes: &[u8]) -> Profile {
    Profile {
        display_name: None,
        public_message: None,
        avatar_url: Some(url.to_owned()),
        avatar_hash: Some(hex::encode(calculate_avatar_hash(bytes))),
        avatar_fingerprint: Some(hex::encode(calculate_dhash_fingerprint(bytes).unwrap())),
        updated_at: None,
    }
}

fn centre(png: &[u8]) -> [u8; 4] {
    let image = image::load_from_memory(png).unwrap().to_rgba8();
    image.get_pixel(image.width() / 2, image.height() / 2).0
}

// ---- the server ----

#[derive(Clone)]
pub(super) struct Route {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    /// Send `Content-Length` (else the body ends at the close).
    length: bool,
    /// Answer only after a minute: a fetch still running.
    stall: bool,
}

impl Route {
    pub(super) fn ok(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            headers: vec![("Content-Type".into(), "image/png".into())],
            body,
            length: true,
            stall: false,
        }
    }

    fn stalled(body: Vec<u8>) -> Self {
        Self {
            stall: true,
            ..Self::ok(body)
        }
    }

    fn redirect(to: &str) -> Self {
        Self {
            status: 302,
            headers: vec![("Location".into(), to.into())],
            body: Vec::new(),
            length: true,
            stall: false,
        }
    }

    fn missing() -> Self {
        Self {
            status: 404,
            headers: Vec::new(),
            body: Vec::new(),
            length: true,
            stall: false,
        }
    }
}

/// A one-request-per-connection HTTP/1.1 server.
pub(super) struct Server {
    addr: SocketAddr,
    routes: Arc<Mutex<HashMap<String, Route>>>,
    /// Request targets, in order.
    hits: Arc<Mutex<Vec<String>>>,
    connections: Arc<AtomicUsize>,
}

async fn serve(
    mut stream: tokio::net::TcpStream,
    routes: Arc<Mutex<HashMap<String, Route>>>,
    hits: Arc<Mutex<Vec<String>>>,
) {
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 << 10 {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => head.extend_from_slice(&buf[..n]),
        }
    }
    let text = String::from_utf8_lossy(&head);
    let target = (text.lines().next().unwrap_or("").split(' ').nth(1))
        .unwrap_or("")
        .to_owned();
    hits.lock().unwrap().push(target.clone());
    let route = (routes.lock().unwrap().get(&target).cloned()).unwrap_or_else(Route::missing);
    if route.stall {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
    let mut out = format!("HTTP/1.1 {} X\r\nConnection: close\r\n", route.status);
    for (k, v) in &route.headers {
        out += &format!("{k}: {v}\r\n");
    }
    if route.length {
        out += &format!("Content-Length: {}\r\n", route.body.len());
    }
    out += "\r\n";
    let _ = stream.write_all(out.as_bytes()).await;
    let _ = stream.write_all(&route.body).await;
    let _ = stream.shutdown().await;
}

impl Server {
    pub(super) async fn start(addr: SocketAddr) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let server = Self {
            addr: listener.local_addr()?,
            routes: Arc::default(),
            hits: Arc::default(),
            connections: Arc::default(),
        };
        let (routes, hits, connections) = (
            Arc::clone(&server.routes),
            Arc::clone(&server.hits),
            Arc::clone(&server.connections),
        );
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                connections.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(serve(stream, Arc::clone(&routes), Arc::clone(&hits)));
            }
        });
        Ok(server)
    }

    pub(super) fn route(&self, path: &str, route: Route) {
        self.routes.lock().unwrap().insert(path.into(), route);
    }

    pub(super) fn url(&self, host: &str, path: &str) -> String {
        format!("http://{host}:{}{path}", self.addr.port())
    }

    /// Connections accepted, whatever was sent on them.
    pub(super) fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    pub(super) fn hits(&self) -> Vec<String> {
        self.hits.lock().unwrap().clone()
    }
}

// ---- the resolver ----

/// Names to answers; a name with several answers gives them in turn (the
/// last one repeats). Records lookups.
#[derive(Default)]
pub(super) struct Script {
    answers: Mutex<HashMap<String, Vec<Vec<IpAddr>>>>,
    lookups: Mutex<Vec<String>>,
}

impl Script {
    pub(super) fn answer(&self, host: &str, answers: Vec<Vec<IpAddr>>) {
        self.answers.lock().unwrap().insert(host.into(), answers);
    }

    fn lookups(&self, host: &str) -> usize {
        self.lookups
            .lock()
            .unwrap()
            .iter()
            .filter(|h| *h == host)
            .count()
    }
}

impl NameResolver for Script {
    fn lookup(
        &self,
        host: String,
        admission: Admission,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<IpAddr>>> + Send>> {
        if admission.admit().is_none() {
            let off = std::io::Error::other("contact pictures are off");
            return Box::pin(async move { Err(off) });
        }
        self.lookups.lock().unwrap().push(host.clone());
        let mut answers = self.answers.lock().unwrap();
        let answer = match answers.get_mut(&host) {
            Some(list) if list.len() > 1 => Ok(list.remove(0)),
            Some(list) => Ok(list[0].clone()),
            None => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no such name",
            )),
        };
        Box::pin(async move { answer })
    }
}

pub(super) fn test_policy() -> FetchPolicy {
    FetchPolicy {
        https_only: false,
        also_public: vec![LOCAL],
    }
}

struct Rig {
    server: Server,
    script: Arc<Script>,
    pipeline: Pipeline,
    pictures: Arc<Pictures>,
    db: Arc<AppDb>,
    // Read only by the Unix tests (file modes, symlinks); `tmp` keeps the directory alive.
    #[cfg_attr(not(unix), allow(dead_code))]
    dir: PathBuf,
    #[cfg_attr(not(unix), allow(dead_code))]
    tmp: tempfile::TempDir,
}

impl Rig {
    async fn new() -> Self {
        let server = Server::start(SocketAddr::new(LOCAL, 0)).await.unwrap();
        let script = Arc::new(Script::default());
        script.answer("good.test", vec![vec![LOCAL]]);
        let tmp = dw_testutil::private_tempdir();
        let dir = tmp.path().join("avatars");
        std::fs::create_dir(&dir).unwrap();
        set_mode(&dir, 0o700);
        let db = Arc::new(AppDb::open_in_memory().unwrap());
        let pictures = Arc::new(Pictures::new(true));
        let pipeline = Pipeline::new(
            test_policy(),
            Arc::clone(&script) as Arc<dyn NameResolver>,
            Some(ThumbDir::open(&dir).unwrap()),
            Arc::clone(&db),
        );
        Self {
            server,
            script,
            pipeline,
            pictures,
            db,
            dir,
            tmp,
        }
    }

    /// A fresh client, so no pooled connection hides a lookup.
    fn fetcher(&self) -> Fetcher {
        Fetcher::new(
            test_policy(),
            Arc::clone(&self.script) as Arc<dyn NameResolver>,
        )
    }

    async fn fetch(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        let url = Url::parse(url).unwrap();
        self.fetcher().fetch(&url, Admission::any()).await
    }

    fn ticket(&self) -> super::avatar::Ticket {
        self.pictures.ticket()
    }

    async fn read(&self, profile: &Profile) -> Result<Option<Vec<u8>>, AvatarError> {
        (self.pipeline)
            .for_profile(profile, AvatarSize::Small, Whose::Other, self.ticket())
            .await
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) {}

// ---- acceptance fixtures ----

/// A picture whose hash matches is shown, as 128 and 256 px
/// PNGs, and served from the cache next time (one request, one lookup).
#[tokio::test(flavor = "multi_thread")]
async fn a_matching_avatar_is_shown_and_cached() {
    let rig = Rig::new().await;
    let bytes = png(300, 200);
    rig.server.route("/me.png", Route::ok(bytes.clone()));
    let profile = profile_for(&rig.server.url("good.test", "/me.png"), &bytes);
    let thumb = rig.read(&profile).await.unwrap().unwrap();
    assert!(avatar_image::is_thumbnail(&thumb, 128));
    let large = (rig.pipeline)
        .for_profile(&profile, AvatarSize::Large, Whose::Other, rig.ticket())
        .await
        .unwrap()
        .unwrap();
    assert!(avatar_image::is_thumbnail(&large, 256));
    assert_eq!(rig.read(&profile).await.unwrap().unwrap(), thumb);
    assert_eq!(rig.server.hits(), vec!["/me.png".to_string()]);
    // The name went through the script: `good.test` exists nowhere else.
    assert_eq!(rig.script.lookups("good.test"), 1);
}

/// Wrong hash: the picture is fetched, but the profile's hash is not its
/// hash, so the read is `avatar.hash_mismatch` (initials) and the URL is
/// not fetched again within the recheck time.
#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_hash_shows_initials() {
    let rig = Rig::new().await;
    let bytes = png(64, 64);
    rig.server.route("/a.png", Route::ok(bytes.clone()));
    let url = rig.server.url("good.test", "/a.png");
    let mut profile = profile_for(&url, &bytes);
    profile.avatar_hash = Some("00".repeat(32));
    assert_eq!(rig.read(&profile).await, Err(AvatarError::HashMismatch));
    assert_eq!(rig.read(&profile).await, Err(AvatarError::HashMismatch));
    assert_eq!(rig.server.hits().len(), 1);
    // A profile citing the same URL with the right hash is shown from the
    // cache: the row is the URL's, the verdict each profile's.
    assert!(
        rig.read(&profile_for(&url, &bytes))
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(rig.server.hits().len(), 1);
}

/// The fingerprint does not veto an exact hash (D6, D32): the mobile
/// wallets' dHash is another algorithm, so even one with every bit off
/// verifies when the SHA-256 is the profile's; a malformed one still does
/// not (D26), nor does a near fingerprint with another hash.
#[tokio::test(flavor = "multi_thread")]
async fn the_fingerprint_does_not_veto_an_exact_hash() {
    let rig = Rig::new().await;
    let bytes = png(64, 64);
    rig.server.route("/f.png", Route::ok(bytes.clone()));
    let mut profile = profile_for(&rig.server.url("good.test", "/f.png"), &bytes);
    let fp = calculate_dhash_fingerprint(&bytes).unwrap();
    profile.avatar_fingerprint = Some(hex::encode(fp.map(|b| !b)));
    assert!(rig.read(&profile).await.unwrap().is_some());
    profile.avatar_fingerprint = Some("zz".repeat(8));
    assert_eq!(rig.read(&profile).await, Err(AvatarError::HashMismatch));
    profile.avatar_fingerprint = Some(hex::encode(fp));
    profile.avatar_hash = Some(hex::encode([7u8; 32]));
    assert_eq!(rig.read(&profile).await, Err(AvatarError::HashMismatch));
}

/// Oversize: a declared length over 5 MiB is refused before reading, and
/// a body without a length is cut off as it passes 5 MiB. A picture over
/// 4096 px a side is refused by the decoder.
#[tokio::test(flavor = "multi_thread")]
async fn oversize_avatars_are_refused() {
    let rig = Rig::new().await;
    let big = vec![0u8; MAX_BYTES + 1];
    rig.server.route("/declared.png", Route::ok(big.clone()));
    rig.server.route(
        "/streamed.png",
        Route {
            length: false,
            ..Route::ok(big)
        },
    );
    rig.server
        .route("/edge.png", Route::ok(vec![0u8; MAX_BYTES]));
    let url = |p: &str| rig.server.url("good.test", p);
    assert_eq!(
        rig.fetch(&url("/declared.png")).await,
        Err(FetchError::TooLarge)
    );
    assert_eq!(
        rig.fetch(&url("/streamed.png")).await,
        Err(FetchError::TooLarge)
    );
    assert_eq!(
        rig.fetch(&url("/edge.png")).await.map(|b| b.len()),
        Ok(MAX_BYTES)
    );
    let profile = profile_for(&url("/declared.png"), &png(8, 8));
    assert_eq!(rig.read(&profile).await, Err(AvatarError::TooLarge));
    // Cached as too large: not fetched again.
    assert_eq!(rig.read(&profile).await, Err(AvatarError::TooLarge));
    assert_eq!(
        rig.server
            .hits()
            .iter()
            .filter(|h| *h == "/declared.png")
            .count(),
        2
    );

    let wide = encode(&solid(MAX_SIDE + 1, 1, [1, 2, 3, 255]), ImageFormat::Png);
    assert_eq!(
        avatar_image::decode(&wide).map(|_| ()),
        Err(AvatarError::TooLarge)
    );
    rig.server.route("/wide.png", Route::ok(wide.clone()));
    let profile = profile_for(&url("/wide.png"), &wide);
    assert_eq!(rig.read(&profile).await, Err(AvatarError::TooLarge));
    let edge = encode(&solid(MAX_SIDE, 1, [0; 4]), ImageFormat::Png);
    assert!(avatar_image::decode(&edge).is_ok());
}

/// A redirect to 10.0.0.1, written as an address, in hex, or behind a name,
/// fails at the guard: nothing connects to it.
#[tokio::test(flavor = "multi_thread")]
async fn a_redirect_to_a_private_address_is_refused() {
    let rig = Rig::new().await;
    rig.script
        .answer("intranet.test", vec![vec!["10.0.0.1".parse().unwrap()]]);
    rig.server
        .route("/lit", Route::redirect("http://10.0.0.1/avatar.png"));
    rig.server
        .route("/name", Route::redirect("http://intranet.test/avatar.png"));
    rig.server
        .route("/hex", Route::redirect("http://0x0a000001/avatar.png"));
    for path in ["/lit", "/name", "/hex"] {
        assert_eq!(
            rig.fetch(&rig.server.url("good.test", path)).await,
            Err(FetchError::Address("10.0.0.1".into())),
            "{path}"
        );
    }
    let profile = profile_for(&rig.server.url("good.test", "/lit"), &png(8, 8));
    assert_eq!(rig.read(&profile).await, Err(AvatarError::FetchFailed));
    // The production policy refuses a private literal before any request.
    let strict = FetchPolicy::STRICT;
    let refused = strict.check_url(&Url::parse("https://10.0.0.1/a.png").unwrap());
    assert_eq!(refused, Err(FetchError::Address("10.0.0.1".into())));
}

/// GIF: an animated GIF is accepted and its first frame (red) is the
/// thumbnail (D9).
#[tokio::test(flavor = "multi_thread")]
async fn a_gif_shows_its_first_frame() {
    let rig = Rig::new().await;
    let gif = animated_gif();
    rig.server.route("/anim.gif", Route::ok(gif.clone()));
    let profile = profile_for(&rig.server.url("good.test", "/anim.gif"), &gif);
    let thumb = rig.read(&profile).await.unwrap().unwrap();
    let [r, g, b, a] = centre(&thumb);
    assert!(
        r > 200 && g < 50 && b < 50 && a == 255,
        "first frame: {:?}",
        [r, g, b, a]
    );
}

// ---- the guard beyond the fixtures ----

/// A rebinding name: public at the first lookup, private at the next. The
/// fetch that got the public answer connected there; the one that got the
/// private answer was refused, and a name with mixed answers is refused
/// whole (D4). A listener on the private address, same port, never sees a
/// connection (Linux, where all of 127/8 is local).
#[tokio::test(flavor = "multi_thread")]
async fn dns_rebinding_never_reaches_a_private_address() {
    let rig = Rig::new().await;
    let bytes = png(16, 16);
    let port = rig.server.addr.port();
    let private = Server::start(SocketAddr::new(PRIVATE, port)).await.ok();
    #[cfg(target_os = "linux")]
    assert!(private.is_some(), "127.0.0.2 must be bindable on Linux");
    rig.server.route("/r.png", Route::ok(bytes.clone()));
    if let Some(p) = &private {
        p.route("/r.png", Route::ok(bytes.clone()));
    }
    rig.script
        .answer("flip.test", vec![vec![LOCAL], vec![PRIVATE]]);
    rig.script
        .answer("mixed.test", vec![vec![LOCAL, "10.1.2.3".parse().unwrap()]]);
    let flip = format!("http://flip.test:{port}/r.png");
    assert_eq!(rig.fetch(&flip).await.map(|b| b.len()), Ok(bytes.len()));
    assert_eq!(
        rig.fetch(&flip).await,
        Err(FetchError::Address(PRIVATE.to_string()))
    );
    assert_eq!(rig.script.lookups("flip.test"), 2);
    assert_eq!(
        rig.fetch(&format!("http://mixed.test:{port}/r.png")).await,
        Err(FetchError::Address("10.1.2.3".into()))
    );
    // Redirects to the private listener, by address and by name.
    rig.script.answer("inside.test", vec![vec![PRIVATE]]);
    rig.server.route(
        "/to-ip",
        Route::redirect(&format!("http://127.0.0.2:{port}/r.png")),
    );
    rig.server.route(
        "/to-name",
        Route::redirect(&format!("http://inside.test:{port}/r.png")),
    );
    for path in ["/to-ip", "/to-name"] {
        let fetched = rig.fetch(&rig.server.url("good.test", path)).await;
        assert_eq!(
            fetched,
            Err(FetchError::Address(PRIVATE.to_string())),
            "{path}"
        );
    }
    if let Some(p) = &private {
        assert_eq!(
            p.connections.load(Ordering::SeqCst),
            0,
            "a private address was reached"
        );
    }
}

/// Redirects: at most 3 are followed, each checked; a fourth fails. A
/// status other than 200 fails and is cached as a failure.
#[tokio::test(flavor = "multi_thread")]
async fn redirects_are_followed_three_times_at_most() {
    let rig = Rig::new().await;
    let bytes = png(8, 8);
    rig.server.route("/end.png", Route::ok(bytes.clone()));
    for (from, to) in [
        ("/r0", "/r1"),
        ("/r1", "/r2"),
        ("/r2", "/r3"),
        ("/r3", "/end.png"),
    ] {
        rig.server
            .route(from, Route::redirect(&rig.server.url("good.test", to)));
    }
    let three = rig.fetch(&rig.server.url("good.test", "/r1")).await;
    assert_eq!(three.map(|b| b.len()), Ok(bytes.len()));
    let four = rig.fetch(&rig.server.url("good.test", "/r0")).await;
    assert_eq!(four, Err(FetchError::Redirects));
    // A relative redirect resolves against the hop.
    rig.server.route("/rel", Route::redirect("/end.png"));
    let relative = rig.fetch(&rig.server.url("good.test", "/rel")).await;
    assert_eq!(relative.map(|b| b.len()), Ok(bytes.len()));
    let missing = profile_for(&rig.server.url("good.test", "/missing.png"), &bytes);
    assert_eq!(rig.read(&missing).await, Err(AvatarError::FetchFailed));
    assert_eq!(rig.read(&missing).await, Err(AvatarError::FetchFailed));
    assert_eq!(
        rig.server
            .hits()
            .iter()
            .filter(|h| *h == "/missing.png")
            .count(),
        1
    );
}

/// What is not PNG, JPEG or GIF is `avatar.unsupported`, whatever the
/// server calls it (D8); a JPEG is fine.
#[tokio::test(flavor = "multi_thread")]
async fn only_png_jpeg_and_gif_are_decoded() {
    let rig = Rig::new().await;
    let jpeg = encode(&picture(40, 30), ImageFormat::Jpeg);
    rig.server.route("/p.jpg", Route::ok(jpeg.clone()));
    let profile = profile_for(&rig.server.url("good.test", "/p.jpg"), &jpeg);
    assert!(rig.read(&profile).await.unwrap().is_some());
    let mut webp = b"RIFF\0\0\0\0WEBPVP8 ".to_vec();
    webp.extend_from_slice(&[0; 64]);
    let svg = b"<svg xmlns='http://www.w3.org/2000/svg'/>".to_vec();
    for body in [webp, svg, b"hello".to_vec(), Vec::new()] {
        assert_eq!(
            avatar_image::ingest(&body, None, false),
            Err(AvatarError::Unsupported)
        );
    }
}

/// Android's crop parameter is not sent, and is applied (D15): the
/// top-left quarter of the quadrant picture is red.
#[tokio::test(flavor = "multi_thread")]
async fn androids_zoom_parameter_crops_the_picture() {
    let rig = Rig::new().await;
    let bytes = png(200, 200);
    rig.server
        .route("/z.png?a=b%20c&k", Route::ok(bytes.clone()));
    let url = (rig.server).url(
        "good.test",
        "/z.png?a=b%20c&dashpay-profile-pic-zoom=0.0,0.0,0.5,0.5&k",
    );
    let thumb = rig.read(&profile_for(&url, &bytes)).await.unwrap().unwrap();
    // The rest of the query is kept as it was written.
    assert_eq!(rig.server.hits(), vec!["/z.png?a=b%20c&k".to_string()]);
    let alone = rig
        .server
        .url("good.test", "/z.png?dashpay-profile-pic-zoom=0,0,1,1");
    assert_eq!(Target::parse(&alone).unwrap().fetch.query(), None);
    let [r, g, b, _] = centre(&thumb);
    assert!(
        r > 120 && g < 90 && b < 90,
        "top-left quadrant: {:?}",
        [r, g, b]
    );
    // Uncropped, the centre is where the four quadrants meet: not red.
    let whole = avatar_image::ingest(&bytes, None, false).unwrap();
    assert_ne!(whole.small, thumb);
    assert_eq!(Crop::from_zoom("0,0,1"), None);
    assert_eq!(Crop::from_zoom("0.5,0,0.5,1"), None);
    assert_eq!(Crop::from_zoom("NaN,0,1,1"), None);
    assert_eq!(Crop::from_zoom("0,0,1.5,1"), None);
    assert!(Crop::from_zoom("0, 0.1, 1, 0.9").is_some());
}

// ---- hash and dHash are the library's ----

#[test]
fn hash_and_dhash_equal_the_librarys() {
    use sha2::Digest as _;
    let fixtures = [
        png(300, 200),
        png(9, 8),
        encode(&picture(50, 70), ImageFormat::Jpeg),
        animated_gif(),
    ];
    let mut checked = 0;
    for bytes in fixtures.iter().chain(corpus().iter().map(|(_, b)| b)) {
        let Ok(ingested) = avatar_image::ingest(bytes, None, true) else {
            continue;
        };
        assert_eq!(ingested.sha, calculate_avatar_hash(bytes));
        assert_eq!(ingested.sha.to_vec(), sha2::Sha256::digest(bytes).to_vec());
        assert_eq!(
            ingested.dhash,
            Some(calculate_dhash_fingerprint(bytes).unwrap())
        );
        // A read skips the second decode (D32).
        assert_eq!(
            avatar_image::ingest(bytes, None, false).unwrap().dhash,
            None
        );
        checked += 1;
    }
    assert!(checked >= 8, "{checked}");
}

// ---- the cache ----

/// Thumbnails are files the cache wrote through its descriptor: 0600, no
/// temporaries left, a symlink planted at a thumbnail's name replaced (not
/// followed), and a thumbnail replaced by a symlink or by garbage fetched
/// again rather than read.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn thumbnails_are_written_through_the_directory_descriptor() {
    use std::os::unix::fs::{MetadataExt, symlink};
    let rig = Rig::new().await;
    let bytes = png(64, 64);
    rig.server.route("/c.png", Route::ok(bytes.clone()));
    let profile = profile_for(&rig.server.url("good.test", "/c.png"), &bytes);
    let ingested = avatar_image::ingest(&bytes, None, false).unwrap();
    let stem = super::avatar_cache::stem(&ingested.sha, None);
    let small = rig.dir.join(format!("{stem}-128.png"));
    // A symlink where the small thumbnail goes, pointing outside.
    let outside = rig.tmp.path().join("outside.png");
    std::fs::write(&outside, b"keep").unwrap();
    symlink(&outside, &small).unwrap();
    assert!(rig.read(&profile).await.unwrap().is_some());
    assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
    for px in [128, 256] {
        let meta = std::fs::symlink_metadata(rig.dir.join(format!("{stem}-{px}.png"))).unwrap();
        assert!(meta.is_file(), "{px}: a regular file, not the symlink");
        assert_eq!(meta.mode() & 0o777, 0o600);
    }
    let names: Vec<String> = (std::fs::read_dir(&rig.dir).unwrap())
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    // Replaced by a symlink to a valid thumbnail elsewhere: not followed;
    // the URL is fetched again and the thumbnail rewritten.
    let elsewhere = rig.tmp.path().join("elsewhere.png");
    std::fs::copy(&small, &elsewhere).unwrap();
    std::fs::remove_file(&small).unwrap();
    symlink(&elsewhere, &small).unwrap();
    assert!(rig.read(&profile).await.unwrap().is_some());
    assert_eq!(rig.server.hits().len(), 2);
    assert!(std::fs::symlink_metadata(&small).unwrap().is_file());
    // Garbage in place of a thumbnail: fetched again too.
    std::fs::write(&small, b"not a png").unwrap();
    assert!(rig.read(&profile).await.unwrap().is_some());
    assert_eq!(rig.server.hits().len(), 3);
    let url_sha = Target::parse(profile.avatar_url.as_deref().unwrap())
        .unwrap()
        .url_sha;
    let row = rig.db.avatar_row(&url_sha).unwrap().unwrap();
    assert_eq!(row.file.as_deref(), Some(stem.as_str()));
}

/// A directory others may write is refused (D10); leftover temporary
/// files are removed at open; a symlink standing in for the directory is
/// the user's choice and followed.
#[cfg(unix)]
#[test]
fn the_cache_directory_must_be_private() {
    let tmp = dw_testutil::private_tempdir();
    let dir = tmp.path().join("avatars");
    std::fs::create_dir(&dir).unwrap();
    set_mode(&dir, 0o770);
    assert!(ThumbDir::open(&dir).is_err());
    set_mode(&dir, 0o702);
    assert!(ThumbDir::open(&dir).is_err());
    set_mode(&dir, 0o700);
    std::fs::write(dir.join(".tmp-0123"), b"x").unwrap();
    std::fs::write(dir.join("keep"), b"x").unwrap();
    ThumbDir::open(&dir).unwrap();
    assert!(!dir.join(".tmp-0123").exists());
    assert!(dir.join("keep").exists());
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&dir, &link).unwrap();
    assert!(ThumbDir::open(&link).is_ok());
}

/// The toggle off (D17): another user's avatar is not read at all, an own
/// identity's comes from the cache only (shown if verified there); nothing
/// is fetched.
#[tokio::test(flavor = "multi_thread")]
async fn with_pictures_off_nothing_is_fetched() {
    let rig = Rig::new().await;
    let bytes = png(32, 32);
    rig.server.route("/o.png", Route::ok(bytes.clone()));
    let profile = profile_for(&rig.server.url("good.test", "/o.png"), &bytes);
    let (pipeline, pictures) = (&rig.pipeline, &rig.pictures);
    let read = |p: Profile, whose| async move {
        let ticket = pictures.ticket();
        pipeline
            .for_profile(&p, AvatarSize::Small, whose, ticket)
            .await
    };
    rig.pictures.set(false);
    assert_eq!(read(profile.clone(), Whose::Own).await, Ok(None));
    assert_eq!(read(profile.clone(), Whose::Other).await, Ok(None));
    assert!(rig.server.hits().is_empty());
    rig.pictures.set(true);
    assert!(read(profile.clone(), Whose::Own).await.unwrap().is_some());
    rig.pictures.set(false);
    assert!(read(profile.clone(), Whose::Own).await.unwrap().is_some());
    assert_eq!(read(profile.clone(), Whose::Other).await, Ok(None));
    let mut wrong = profile.clone();
    wrong.avatar_hash = Some("11".repeat(32));
    assert_eq!(read(wrong, Whose::Own).await, Ok(None));
    assert_eq!(rig.server.hits().len(), 1);
}

/// Pauses the first read reaching `at` until released: `(reached,
/// release)`. Reads past it, and other points, go on.
pub(super) fn pause_at(
    pictures: &Pictures,
    at: &'static str,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
) {
    let (reached, reached_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release) = tokio::sync::oneshot::channel::<()>();
    let once = Mutex::new(Some((reached, release)));
    pictures.set_hook(Box::new(move |point| {
        if point != at {
            return None;
        }
        let (reached, release) = once.lock().unwrap().take()?;
        let _ = reached.send(());
        Some(Box::pin(async move {
            let _ = release.await;
        }))
    }));
    (reached_rx, release_tx)
}

/// `read` (of `/slow.png`), with the toggle turned off once its fetch has
/// reached the server.
async fn off_mid_fetch<T: std::fmt::Debug>(rig: &Rig, read: impl Future<Output = T>) -> T {
    let hits = || {
        rig.server
            .hits()
            .iter()
            .filter(|h| *h == "/slow.png")
            .count()
    };
    let mut read = std::pin::pin!(read);
    let (before, deadline) = (hits(), Instant::now() + Duration::from_secs(30));
    while hits() == before {
        assert!(Instant::now() < deadline, "the fetch never started");
        tokio::select! {
            read = &mut read => panic!("the read ended before its fetch: {read:?}"),
            () = tokio::time::sleep(Duration::from_millis(10)) => {}
        }
    }
    rig.pictures.set(false);
    read.await
}

/// An off holds for reads already running (D33): one that began with the
/// toggle on and was paused at the cache lookup (no row) does not fetch;
/// one paused at a verified cache hit answers `None` for another user
/// (an own identity's still shows: the cache is allowed while off); one
/// whose fetch is running has it abandoned, and nothing is recorded.
#[tokio::test(flavor = "multi_thread")]
async fn an_off_holds_for_reads_already_running() {
    let rig = Rig::new().await;
    let bytes = png(24, 24);
    rig.server.route("/a.png", Route::ok(bytes.clone()));
    let profile = profile_for(&rig.server.url("good.test", "/a.png"), &bytes);
    let pictures = &rig.pictures;
    let off_while_paused = |at| {
        let (reached, release) = pause_at(pictures, at);
        async move {
            reached.await.unwrap();
            pictures.set(false);
            release.send(()).unwrap();
        }
    };
    // Paused at the lookup, nothing cached: no fetch.
    let (read, ()) = tokio::join!(rig.read(&profile), off_while_paused("row"));
    assert_eq!(read, Ok(None));
    assert!(rig.server.hits().is_empty());
    // Cached (on), then paused at the hit.
    rig.pictures.set(true);
    assert!(rig.read(&profile).await.unwrap().is_some());
    let (read, ()) = tokio::join!(rig.read(&profile), off_while_paused("thumbnail"));
    assert_eq!(read, Ok(None));
    rig.pictures.set(true);
    let own = async {
        let ticket = rig.ticket();
        (rig.pipeline)
            .for_profile(&profile, AvatarSize::Small, Whose::Own, ticket)
            .await
    };
    let (read, ()) = tokio::join!(own, off_while_paused("thumbnail"));
    assert!(read.unwrap().is_some());
    assert_eq!(rig.server.hits().len(), 1);
    // A fetch running when the toggle goes off is abandoned at once.
    rig.pictures.set(true);
    let slow = png(25, 25);
    rig.server.route("/slow.png", Route::stalled(slow.clone()));
    let stalled = profile_for(&rig.server.url("good.test", "/slow.png"), &slow);
    let started = Instant::now();
    assert_eq!(off_mid_fetch(&rig, rig.read(&stalled)).await, Ok(None));
    // Not the stall's 60 s: the fetch was abandoned.
    assert!(started.elapsed() < Duration::from_secs(45));
    let key = Target::parse(&stalled.avatar_url.clone().unwrap())
        .unwrap()
        .url_sha;
    assert_eq!(rig.db.avatar_row(&key).unwrap(), None, "nothing recorded");
    // An own identity's fetch is abandoned the same way (off: cache only).
    rig.pictures.set(true);
    let own = async {
        let ticket = rig.ticket();
        (rig.pipeline)
            .for_profile(&stalled, AvatarSize::Small, Whose::Own, ticket)
            .await
    };
    assert_eq!(off_mid_fetch(&rig, own).await, Ok(None));
    assert_eq!(rig.db.avatar_row(&key).unwrap(), None);
}

/// An off returns only once every read that began before it has ended
/// (D36). A read paused inside a fetch step, after its revocation was
/// first polled and before the fetch was, makes no request, and the off
/// waits for it to end; so does a read paused where it cannot be revoked
/// (an own identity's at a cache hit).
#[tokio::test(flavor = "multi_thread")]
async fn an_off_returns_once_the_reads_before_it_ended() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let rig = Rig::new().await;
    let pictures = &rig.pictures;
    let off = || async {
        let turn = pictures.begin_transition().await;
        turn.apply(false, async { Ok::<_, ()>(()) }).await
    };
    for (path, whose) in [("/o.png", Whose::Other), ("/w.png", Whose::Own)] {
        pictures.set(true);
        let bytes = png(26, 26);
        rig.server.route(path, Route::ok(bytes.clone()));
        let profile = profile_for(&rig.server.url("good.test", path), &bytes);
        let (reached, release) = pause_at(pictures, "fetch");
        let ended = AtomicBool::new(false);
        let read = async {
            let ticket = rig.ticket();
            let read = (rig.pipeline)
                .for_profile(&profile, AvatarSize::Small, whose, ticket)
                .await;
            ended.store(true, Ordering::SeqCst);
            read
        };
        let off = async {
            reached.await.unwrap();
            off().await.unwrap();
            assert!(
                ended.load(Ordering::SeqCst),
                "{whose:?}: off returned first"
            );
            let _ = release.send(());
        };
        let (read, ()) = tokio::join!(read, off);
        assert_eq!(read, Ok(None), "{whose:?}");
        assert!(
            rig.server.hits().is_empty(),
            "{whose:?}: a request was made"
        );
    }
    // Cached, an own identity's read paused at the hit holds the off.
    pictures.set(true);
    let bytes = png(27, 27);
    rig.server.route("/c.png", Route::ok(bytes.clone()));
    let profile = profile_for(&rig.server.url("good.test", "/c.png"), &bytes);
    assert!(rig.read(&profile).await.unwrap().is_some());
    let (reached, release) = pause_at(pictures, "thumbnail");
    let own = async {
        let ticket = rig.ticket();
        (rig.pipeline)
            .for_profile(&profile, AvatarSize::Small, Whose::Own, ticket)
            .await
    };
    let held = async {
        reached.await.unwrap();
        let mut off = std::pin::pin!(off());
        let early = tokio::time::timeout(Duration::from_millis(300), &mut off);
        assert!(early.await.is_err(), "off returned while a read ran");
        release.send(()).unwrap();
        off.await.unwrap();
    };
    let (read, ()) = tokio::join!(own, held);
    assert!(read.unwrap().is_some());
    assert!(!pictures.on());
}

/// Transitions at once apply in turn (D34): one paused between its durable
/// write and memory holds the next back, so memory ends as the last value
/// written, not as the slower call's.
#[tokio::test(flavor = "multi_thread")]
async fn overlapping_transitions_end_at_the_last_durable_write() {
    let pictures = Arc::new(Pictures::new(false));
    let durable = Mutex::new(Vec::new());
    let (go, wait) = tokio::sync::oneshot::channel::<()>();
    let slow_on = async {
        let write = async {
            durable.lock().unwrap().push(true);
            let _ = wait.await;
            Ok::<_, ()>(())
        };
        pictures.begin_transition().await.apply(true, write).await
    };
    let off = async {
        tokio::task::yield_now().await;
        let write = async {
            durable.lock().unwrap().push(false);
            Ok::<_, ()>(())
        };
        pictures.begin_transition().await.apply(false, write).await
    };
    let release = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        go.send(()).unwrap();
    };
    let (a, b, ()) = tokio::join!(slow_on, off, release);
    assert_eq!((a, b), (Ok(()), Ok(())));
    assert_eq!(*durable.lock().unwrap(), [true, false]);
    assert!(!pictures.on());
    // A failed write leaves memory as it was.
    let failed = (pictures.begin_transition().await).apply(true, async { Err::<(), _>("disk") });
    assert_eq!(failed.await, Err("disk"));
    assert!(!pictures.on());
}

/// One fetch per URL at a time (D12): reads of one avatar at once make one
/// request, and every one of them is answered.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_reads_of_one_url_fetch_it_once() {
    let rig = Rig::new().await;
    let bytes = png(48, 48);
    rig.server.route("/same.png", Route::ok(bytes.clone()));
    let profile = profile_for(&rig.server.url("good.test", "/same.png"), &bytes);
    let reads = (0..8).map(|_| rig.read(&profile));
    for read in futures_util::future::join_all(reads).await {
        assert!(read.unwrap().is_some());
    }
    assert_eq!(rig.server.hits().len(), 1);
}

/// A profile whose hash or fingerprint could match nothing is a mismatch
/// without a request.
#[tokio::test(flavor = "multi_thread")]
async fn an_unverifiable_profile_is_not_fetched() {
    let rig = Rig::new().await;
    let bytes = png(16, 16);
    rig.server.route("/n.png", Route::ok(bytes.clone()));
    let good = profile_for(&rig.server.url("good.test", "/n.png"), &bytes);
    for broken in [
        Profile {
            avatar_hash: None,
            ..good.clone()
        },
        Profile {
            avatar_fingerprint: Some("zz".repeat(8)),
            ..good.clone()
        },
        Profile {
            avatar_hash: Some("ab".repeat(31)),
            ..good.clone()
        },
    ] {
        assert_eq!(rig.read(&broken).await, Err(AvatarError::HashMismatch));
    }
    assert!(rig.server.hits().is_empty());
}

/// Without a cache directory the thumbnails are kept in memory, so a
/// verified picture is not fetched on every read.
#[tokio::test(flavor = "multi_thread")]
async fn without_a_directory_thumbnails_stay_in_memory() {
    let rig = Rig::new().await;
    let bytes = png(20, 20);
    rig.server.route("/m.png", Route::ok(bytes.clone()));
    let pipeline = Pipeline::new(
        test_policy(),
        Arc::clone(&rig.script) as Arc<dyn NameResolver>,
        None,
        Arc::new(AppDb::open_in_memory().unwrap()),
    );
    let profile = profile_for(&rig.server.url("good.test", "/m.png"), &bytes);
    for size in [AvatarSize::Small, AvatarSize::Small, AvatarSize::Large] {
        let png = pipeline
            .for_profile(&profile, size, Whose::Other, rig.ticket())
            .await
            .unwrap()
            .unwrap();
        assert!(avatar_image::is_thumbnail(&png, size.px()));
    }
    assert_eq!(rig.server.hits().len(), 1);
}

/// Rotation unlinks the files of the rows it drops: an old row whose
/// thumbnails fill the budget goes when a new picture is cached.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn rotation_unlinks_the_dropped_rows_files() {
    let rig = Rig::new().await;
    let old_stem = "0".repeat(64);
    for px in [128, 256] {
        std::fs::write(rig.dir.join(format!("{old_stem}-{px}.png")), b"old").unwrap();
    }
    rig.db
        .put_avatar_row(&dw_appdb::AvatarRow {
            url_sha: "1".repeat(64),
            content_sha: Some("2".repeat(64)),
            dhash: Some("3".repeat(16)),
            status: "ok".into(),
            file: Some(old_stem.clone()),
            fetched_at: 1,
            bytes: Some(200_000_000),
        })
        .unwrap();
    let bytes = png(24, 24);
    rig.server.route("/new.png", Route::ok(bytes.clone()));
    let profile = profile_for(&rig.server.url("good.test", "/new.png"), &bytes);
    assert!(rig.read(&profile).await.unwrap().is_some());
    assert!(rig.db.avatar_row(&"1".repeat(64)).unwrap().is_none());
    for px in [128, 256] {
        assert!(
            !rig.dir.join(format!("{old_stem}-{px}.png")).exists(),
            "{px}"
        );
    }
    assert!(rig.read(&profile).await.unwrap().is_some());
    assert_eq!(rig.server.hits().len(), 1);
}

// ---- sources ----

#[tokio::test(flavor = "multi_thread")]
async fn a_url_source_is_fetched_now() {
    let rig = Rig::new().await;
    let bytes = png(40, 40);
    rig.server.route("/u.png", Route::ok(bytes.clone()));
    let url = rig.server.url("good.test", "/u.png");
    let (published, ingested) = rig.pipeline.prepare_url(&format!(" {url} ")).await.unwrap();
    assert_eq!(published, url);
    assert_eq!(ingested.sha, calculate_avatar_hash(&bytes));
    // The read that follows publishing it is a cache hit.
    assert!(
        rig.read(&profile_for(&url, &bytes))
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(rig.server.hits().len(), 1);
    // The strict policy refuses what is not https on 443 as an argument
    // error, and a private address as a failed fetch.
    let strict = Pipeline::new(
        FetchPolicy::STRICT,
        Arc::clone(&rig.script) as Arc<dyn NameResolver>,
        None,
        Arc::clone(&rig.db),
    );
    for bad in [
        "http://example.com/a.png",
        "https://example.com:8080/a.png",
        "https://user@example.com/a.png",
        "not a url",
    ] {
        let code = strict.prepare_url(bad).await.unwrap_err().code();
        assert_eq!(code, "invalid_argument", "{bad}");
    }
    let long = format!("https://example.com/{}", "a".repeat(2048));
    assert_eq!(
        strict.prepare_url(&long).await.unwrap_err().code(),
        "invalid_argument"
    );
    assert_eq!(
        strict.prepare_url("https://10.0.0.1/a.png").await.map(drop),
        Err(AvatarError::FetchFailed)
    );
}

#[test]
fn a_file_source_is_cropped_and_reencoded() {
    // The upload is a PNG of the crop, at most 512 px a side, and the
    // candidate's hash is the upload's (D18).
    let jpeg = encode(&picture(1200, 800), ImageFormat::Jpeg);
    let crop = CropRect {
        x: 0,
        y: 0,
        width: 600,
        height: 400,
    };
    let (upload, ingested) = avatar_image::ingest_file(&jpeg, Some(crop)).unwrap();
    assert_eq!(avatar_image::sniff(&upload), Some(ImageFormat::Png));
    let decoded = image::load_from_memory(&upload).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (512, 341));
    assert_eq!(ingested.sha, calculate_avatar_hash(&upload));
    let outside = CropRect { x: 1000, ..crop };
    let refused = avatar_image::ingest_file(&jpeg, Some(outside)).unwrap_err();
    assert_eq!(refused.code(), "invalid_argument");
    let small = encode(&picture(100, 60), ImageFormat::Png);
    let (upload, _) = avatar_image::ingest_file(&small, None).unwrap();
    let decoded = image::load_from_memory(&upload).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (100, 60));
}

#[test]
fn gravatar_urls_are_androids_of_the_normalised_address() {
    use super::avatar::gravatar_url as gravatar;
    let url =
        "https://www.gravatar.com/avatar/55502f40dc8b7c769880b10874abc9d0?s=200&d=404&rating=g";
    assert_eq!(gravatar("test@example.com").unwrap(), url);
    assert_eq!(gravatar("  Test@Example.COM \n").unwrap(), url);
    for bad in ["", "no-at", "a@b", "a b@c.de", "@c.de", "a@.de", "a@c.de."] {
        assert_eq!(
            gravatar(bad).unwrap_err().code(),
            "invalid_argument",
            "{bad:?}"
        );
    }
}

// ---- the decoder's seed corpus ----

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/avatar-corpus")
}

fn corpus() -> Vec<(String, Vec<u8>)> {
    let mut seeds: Vec<_> = (std::fs::read_dir(corpus_dir()).unwrap())
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().into_string().unwrap(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    seeds.sort();
    seeds
}

/// Every seed, and deterministic mutations of it (bit flips, truncations,
/// runs of 0xff over size fields), decodes or fails cleanly: no panic (the
/// decoder is called unguarded here) and no picture past the limits.
#[test]
fn the_decoder_survives_its_seed_corpus() {
    let seeds = corpus();
    assert!(seeds.len() >= 10, "seeds: {}", seeds.len());
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state as usize
    };
    let mut decoded = 0;
    for (name, seed) in &seeds {
        let mut cases = vec![seed.clone()];
        for _ in 0..40 {
            if seed.is_empty() {
                break;
            }
            let mut m = seed.clone();
            let at = next() % m.len();
            match next() % 3 {
                0 => m[at] ^= 1 << (next() % 8),
                1 => m.truncate(at),
                _ => m.iter_mut().skip(at).take(4).for_each(|b| *b = 0xff),
            }
            cases.push(m);
        }
        for case in cases {
            if let Ok(image) = avatar_image::decode(&case) {
                assert!(
                    image.width() <= MAX_SIDE && image.height() <= MAX_SIDE,
                    "{name}"
                );
                decoded += 1;
            }
        }
    }
    assert!(decoded >= 8, "the valid seeds decode ({decoded})");
}

/// Regenerates the seed corpus (`DW_AVATAR_CORPUS=1`); the files are
/// committed, so this does nothing unless asked.
#[test]
fn write_the_seed_corpus() {
    if std::env::var_os("DW_AVATAR_CORPUS").is_none() {
        return;
    }
    let dir = corpus_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let gray16 = DynamicImage::ImageLuma16(image::ImageBuffer::from_fn(8, 8, |x, y| {
        image::Luma([(x * y * 1000) as u16])
    }));
    let mut gray16_png = Vec::new();
    gray16
        .write_to(&mut Cursor::new(&mut gray16_png), ImageFormat::Png)
        .unwrap();
    // A PNG whose header claims 65535 x 65535 (CRC fixed up), and a GIF
    // whose screen claims the same.
    let mut huge = png(2, 2);
    huge[16..20].copy_from_slice(&65535u32.to_be_bytes());
    huge[20..24].copy_from_slice(&65535u32.to_be_bytes());
    let crc = crc32(&huge[12..29]);
    huge[29..33].copy_from_slice(&crc.to_be_bytes());
    // A JPEG whose frame header (SOF0) claims 65535 x 65535.
    let mut huge_jpeg = encode(&picture(8, 8), ImageFormat::Jpeg);
    let sof = huge_jpeg
        .windows(2)
        .position(|w| w == [0xFF, 0xC0])
        .unwrap();
    huge_jpeg[sof + 5..sof + 9].copy_from_slice(&[0xff; 4]);
    let mut huge_gif = encode(&picture(4, 4), ImageFormat::Gif);
    huge_gif[6..10].copy_from_slice(&[0xff; 4]);
    let seeds: Vec<(&str, Vec<u8>)> = vec![
        ("png-rgba-16x12.png", png(16, 12)),
        ("png-1x1.png", png(1, 1)),
        ("png-gray16-8x8.png", gray16_png),
        (
            "png-wide-4097x1.png",
            encode(&solid(MAX_SIDE + 1, 1, [9, 9, 9, 255]), ImageFormat::Png),
        ),
        ("png-claims-65535.png", huge),
        ("png-truncated.png", png(16, 16)[..40].to_vec()),
        (
            "jpeg-24x16.jpg",
            encode(&picture(24, 16), ImageFormat::Jpeg),
        ),
        ("jpeg-9x8.jpg", encode(&picture(9, 8), ImageFormat::Jpeg)),
        ("jpeg-claims-65535.jpg", huge_jpeg),
        ("gif-animated.gif", animated_gif()),
        (
            "gif-static-10x10.gif",
            encode(&picture(10, 10), ImageFormat::Gif),
        ),
        ("gif-claims-65535.gif", huge_gif),
        ("not-an-image.bin", b"GIF89a but not really".to_vec()),
    ];
    for (name, bytes) in seeds {
        std::fs::write(dir.join(name), bytes).unwrap();
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// The system resolver answers a name, on a blocking thread (D36: one
/// given up on before a thread takes it is not made).
#[tokio::test(flavor = "multi_thread")]
async fn the_system_resolver_resolves_a_name() {
    let ips = super::avatar_fetch::BlockingResolver::system()
        .lookup("localhost".into(), Admission::any())
        .await
        .unwrap();
    assert!(ips.iter().any(IpAddr::is_loopback), "{ips:?}");
}

/// A read's name lookup is a registered read (D39): one running when the
/// toggle goes off holds the off until it returns, and one whose worker
/// starts after the off is not made.
#[tokio::test(flavor = "multi_thread")]
async fn an_off_waits_for_a_running_lookup_and_stops_a_later_one() {
    use super::avatar_fetch::BlockingResolver;
    use std::sync::mpsc;
    let pictures = Arc::new(Pictures::new(true));
    let admission = || pictures.ticket().admission();
    let made = Arc::new(Mutex::new(Vec::<String>::new()));
    let (reached_tx, reached) = mpsc::channel::<()>();
    let (release, release_rx) = mpsc::channel::<()>();
    let (release_rx, recorded) = (Mutex::new(release_rx), Arc::clone(&made));
    // The worker pauses after it was admitted, before the lookup.
    let held = BlockingResolver::new(move |host| {
        reached_tx.send(()).unwrap();
        release_rx.lock().unwrap().recv().unwrap();
        recorded.lock().unwrap().push(host.to_owned());
        Ok(vec![LOCAL])
    });
    let lookup = tokio::spawn(held.lookup("held.test".into(), admission()));
    let reached = tokio::task::spawn_blocking(move || reached.recv());
    reached.await.unwrap().unwrap();
    let mut off = std::pin::pin!(async {
        let turn = pictures.begin_transition().await;
        turn.apply(false, async { Ok::<_, ()>(()) }).await
    });
    let early = tokio::time::timeout(Duration::from_millis(300), &mut off);
    assert!(early.await.is_err(), "off returned while a lookup ran");
    release.send(()).unwrap();
    off.await.unwrap();
    assert_eq!(lookup.await.unwrap().unwrap(), [LOCAL]);
    assert_eq!(*made.lock().unwrap(), ["held.test"]);
    // Admitted under a read that began on, the worker starting after an
    // off: no lookup.
    pictures.set(true);
    let late = admission();
    pictures.set(false);
    let recorded = Arc::clone(&made);
    let quiet = BlockingResolver::new(move |host| {
        recorded.lock().unwrap().push(host.to_owned());
        Ok(vec![LOCAL])
    });
    assert!(quiet.lookup("late.test".into(), late).await.is_err());
    assert_eq!(
        *made.lock().unwrap(),
        ["held.test"],
        "no lookup after the off"
    );
}
