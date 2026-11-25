use serde_json::Value;

use super::*;
use crate::deeplink::{DeepLink, base58, classify_link};

const VECTORS: &str = include_str!("../testdata/app_connect/app-connect-vectors.json");
const FORGE_VECTORS: &str = include_str!("../testdata/app_connect/forge-vectors.json");

/// The DIP's "Request Vector" as a `connect` URI on devnet, with its return link.
const CONNECT_VECTOR_URI: &str = "dashpay://connect?v=2&n=d&r=AgPoPLkn6F-WrlCpXKO-Icx6N1UTmpQbFdmbAa9P7PDScPiDPHotWKuBbCSukV_iqpRnZYi0Z7oenyskwaPff9t4BVlhcHByAjICAbBZDlkmnGiFrPcdJ9386FGH4syXNsQulYEnzTW-bTjoAAAAC6Q7dAAAAAGi9760AAI6AQKiobSsb-8i6ioaaOgSNkSzV4dfa0EsGBCSgcFG57JxvAAAAAA7msoAAAABove-tAAHcHJvZmlsZQMQppsbT2ZmgUU97Dc0ZnpasgQh-IM8ei1Yq4FsJK6RX-KqlGdliLRnuh6fKyTBo99_23gBBggAAAGhKEJcAA&cb=https%3A%2F%2Fyap.pr%2Fconnect%3Fr%3Dd77dc0da9df83a47755a15c42c5d9db9d7a7fd62";
const CONNECT_VECTOR_CALLBACK: &str =
    "https://yap.pr/connect?r=d77dc0da9df83a47755a15c42c5d9db9d7a7fd62";
const CREATED_AT: u64 = 1_791_676_800_000;
const EXPIRES_AT: u64 = 1_799_452_800_000;
const MINUTE: u64 = 60 * 1000;

fn vectors() -> Value {
    serde_json::from_str(VECTORS).unwrap()
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn vector_hex(path: &[&str]) -> Vec<u8> {
    hex(vector_str(path).as_str())
}

fn vector_str(path: &[&str]) -> String {
    let doc = vectors();
    let mut value = &doc;
    for key in path {
        value = &value[*key];
    }
    value.as_str().unwrap().to_owned()
}

fn id58(path: &[&str]) -> [u8; 32] {
    dashcore::base58::decode(&vector_str(path))
        .unwrap()
        .try_into()
        .unwrap()
}

fn request_bytes() -> Vec<u8> {
    vector_hex(&["request", "bytes"])
}

fn renew_bytes() -> Vec<u8> {
    vector_hex(&["renewRequest", "bytes"])
}

fn request_uri() -> String {
    vector_str(&["request", "uri"])
}

fn renew_uri() -> String {
    vector_str(&["renewRequest", "uri"])
}

fn app_eph_pub() -> [u8; 33] {
    vector_hex(&["request", "appEphemeralPubKey"])
        .try_into()
        .unwrap()
}

fn app_id() -> [u8; 32] {
    id58(&["inputs", "appId"])
}

fn group_id() -> [u8; 32] {
    id58(&["inputs", "groupId"])
}

fn connect_vector_r() -> &'static str {
    let r = CONNECT_VECTOR_URI.split("&r=").nth(1).unwrap();
    r.split("&cb=").next().unwrap()
}

fn connect_uri(r: &str, query: &str) -> String {
    format!("dashpay://connect?{query}&r={r}")
}

fn connect_rest() -> &'static str {
    &CONNECT_VECTOR_URI["dashpay://connect".len()..]
}

fn tlv(tlv_type: u8, value: &[u8]) -> Vec<u8> {
    [&[tlv_type, value.len() as u8][..], value].concat()
}

fn u64b(value: u64) -> [u8; 8] {
    value.to_be_bytes()
}

/// The renew request's header: version, key, app id and the label `Yappr`.
fn header() -> Vec<u8> {
    renew_bytes()[..1 + 33 + 32 + 1 + 5].to_vec()
}

fn group_binding() -> Vec<u8> {
    tlv(
        0x02,
        &[&[2, 1][..], &group_id(), &u64b(50_000_000_000), &u64b(0)].concat(),
    )
}

fn created_at_tlv() -> Vec<u8> {
    tlv(0x06, &u64b(CREATED_AT))
}

fn auth_binding(kind: u8, level: u8, id: u8, budget: u64, tail: &[u8]) -> Vec<u8> {
    tlv(
        0x02,
        &[&[kind, level][..], &[id; 32], &u64b(budget), &u64b(0), tail].concat(),
    )
}

fn binding(kind: u8, level: u8, id: u8) -> Vec<u8> {
    auth_binding(kind, level, id, 1, &[])
}

fn name(text: &str) -> Vec<u8> {
    name_bytes(text.as_bytes())
}

fn name_bytes(bytes: &[u8]) -> Vec<u8> {
    [&[bytes.len() as u8][..], bytes].concat()
}

fn encryption(id: u8, purpose: u8) -> Vec<u8> {
    tlv(0x04, &[&[id; 32][..], &[purpose]].concat())
}

fn unknown_tlv() -> Vec<u8> {
    tlv(0x08, &[])
}

fn decode_parts(parts: &[Vec<u8>]) -> Result<AppConnectRequest, Refusal> {
    AppConnectRequest::decode(&[header(), parts.concat()].concat())
}

#[track_caller]
fn refused(result: Result<impl std::fmt::Debug, Refusal>) -> Refusal {
    match result {
        Ok(value) => panic!("expected a refusal, got {value:?}"),
        Err(refusal) => refusal,
    }
}

#[track_caller]
fn assert_invalid(parts: &[Vec<u8>]) {
    assert_eq!(refused(decode_parts(parts)).status, Status::Invalid);
}

#[track_caller]
fn assert_unsupported(parts: &[Vec<u8>]) {
    assert_eq!(refused(decode_parts(parts)).status, Status::Unsupported);
}

#[track_caller]
fn assert_uri_invalid(uri: &str) {
    let refusal = refused(parse(uri));
    assert_eq!(refusal.status, Status::Invalid, "{uri}: {refusal}");
}

#[track_caller]
fn assert_refusal(result: Result<ConnectUri, Refusal>, status: Status, detail: &str) {
    assert_eq!(
        refused(result),
        Refusal {
            status,
            detail: detail.to_owned()
        }
    );
}

fn expected_vector_request() -> AppConnectRequest {
    AppConnectRequest {
        app_eph_pub: app_eph_pub(),
        app_id: app_id(),
        label: "Yappr".into(),
        identity_hint: None,
        auth_bindings: vec![
            AuthBinding {
                bounds: Bounds::Group(group_id()),
                level: Level::Critical,
                budget: Some(50_000_000_000),
                expires_at_ms: Some(EXPIRES_AT),
            },
            AuthBinding {
                bounds: Bounds::DocumentType(
                    id58(&["inputs", "dashpayContractId"]),
                    "profile".into(),
                ),
                level: Level::High,
                budget: Some(1_000_000_000),
                expires_at_ms: Some(EXPIRES_AT),
            },
        ],
        connection_id: Some(vector_hex(&["inputs", "connectionId"]).try_into().unwrap()),
        enc_bindings: vec![EncBinding {
            contract: app_id(),
            purpose: EncPurpose::Encryption,
        }],
        renew: false,
        created_at_ms: CREATED_AT,
    }
}

#[test]
fn the_vector_inputs_are_the_ones_ios_pins() {
    assert_eq!(
        to_hex(&app_id()),
        "f8833c7a2d58ab816c24ae915fe2aa94676588b467ba1e9f2b24c1a3df7fdb78"
    );
    assert_eq!(
        to_hex(&group_id()),
        "b0590e59269c6885acf71d27ddfce85187e2cc9736c42e958127cd35be6d38e8"
    );
    assert_eq!(
        to_hex(&id58(&["inputs", "dashpayContractId"])),
        "a2a1b4ac6fef22ea2a1a68e8123644b357875f6b412c18109281c146e7b271bc"
    );
    assert_eq!(
        to_hex(&id58(&["inputs", "ownerId"])),
        "9296d642338cb3bfceeb554285462719e15f004d0579ce53ea77fb6d86cd9ab9"
    );
    assert_eq!(vectors()["inputs"]["createdAt"].as_u64(), Some(CREATED_AT));
    assert_eq!(vectors()["inputs"]["expiresAt"].as_u64(), Some(EXPIRES_AT));
    assert_eq!(request_bytes().len(), 247);
}

#[test]
fn decodes_the_published_request_field_by_field() {
    let request = AppConnectRequest::decode(&request_bytes()).unwrap();
    assert_eq!(request, expected_vector_request());
    assert_eq!(request.check_supported(), Ok(()));
}

#[test]
fn the_published_requests_encode_back_byte_for_byte() {
    for bytes in [request_bytes(), renew_bytes()] {
        assert_eq!(AppConnectRequest::decode(&bytes).unwrap().encode(), bytes);
    }
    assert_eq!(expected_vector_request().encode(), request_bytes());
}

#[test]
fn request_hash_and_session_leaves_match_the_dip() {
    let request = AppConnectRequest::decode(&request_bytes()).unwrap();
    assert_eq!(
        to_hex(&request.request_hash()),
        vector_str(&["request", "appEphemeralPubKeyHash"])
    );
    assert_eq!(
        to_hex(&request.request_hash()),
        "d77dc0da9df83a47755a15c42c5d9db9d7a7fd62"
    );
    assert_eq!(
        to_hex(&request.session_leaf(0)),
        "a0ad9d4647bd9296611258b8f47db92b06bdaee8782fd28e51632403d8a66a1e"
    );
    assert_eq!(
        to_hex(&request.session_leaf(1)),
        "27a2c59bcf28894aee77c809c3c0e94201db08b66648c16fdfa689e6926b4f52"
    );
    let reserved = sha256d::Hash::hash(&request.app_eph_pub).to_byte_array();
    assert_eq!(
        to_hex(&reserved),
        "18f020830ef82d97c975df563a075665dc2679003ce6f985a4448a0dc607234a"
    );
    assert!((0..=3).all(|i| request.session_leaf(i) != reserved));
}

#[test]
fn parses_the_published_dash_key_uri() {
    let parsed = parse(&request_uri()).unwrap();
    assert_eq!(parsed.carrier, Carrier::DashKey);
    assert_eq!(parsed.network, NetworkSel::Devnet);
    assert_eq!(parsed.request, expected_vector_request());
    assert_eq!(parsed.callback.unwrap().url(), CONNECT_VECTOR_CALLBACK);
}

#[test]
fn parses_the_published_renew_request() {
    let parsed = parse(&renew_uri()).unwrap();
    assert_eq!(
        parsed.request,
        AppConnectRequest::decode(&renew_bytes()).unwrap()
    );
    assert_eq!(parsed.network, NetworkSel::Testnet);
    assert_eq!(parsed.callback, None);
    assert!(parsed.request.renew);
    assert_eq!(
        parsed.request.identity_hint,
        Some(id58(&["inputs", "ownerId"]))
    );
    let bounds: Vec<_> = parsed
        .request
        .auth_bindings
        .iter()
        .map(|b| &b.bounds)
        .collect();
    assert_eq!(bounds, [&Bounds::Group(group_id())]);
    assert!(parsed.request.enc_bindings.is_empty());
}

#[test]
fn refuses_a_mainnet_dash_key_uri() {
    let mainnet = renew_uri().replace("?n=t&", "?n=m&");
    assert_ne!(mainnet, renew_uri());
    assert_refusal(
        parse(&mainnet),
        Status::Invalid,
        "dash-key request v2 is not accepted on mainnet",
    );
}

#[test]
fn a_version_1_dash_key_uri_is_unsupported() {
    let payload = [
        &[0x01][..],
        &app_eph_pub(),
        &[0xcd; 32],
        &[14],
        b"Login to Yappr",
    ]
    .concat();
    let uri = format!("dash-key:{}?n=t&v=1", base58(&payload));
    assert_refusal(parse(&uri), Status::Unsupported, "URI version 1");
    assert!(!is_connect_link(&uri));
}

#[test]
fn dash_key_query_rules() {
    let base = renew_uri();
    assert!(parse(&format!("{base}&x=1&x=2")).is_ok());
    for repeated in [
        "&n=t",
        "&v=2",
        "&cb=https%3A%2F%2Fa.example&cb=https%3A%2F%2Fa.example",
    ] {
        assert_uri_invalid(&format!("{base}{repeated}"));
    }
    let long = format!(
        "https%3A%2F%2Fyap.pr%2F{}",
        "a".repeat(513 - "https://yap.pr/".len())
    );
    for callback in [
        "http%3A%2F%2Fyap.pr%2Fconnect",
        "https%3A%2F%2Fuser%3Apw%40yap.pr%2F",
        "https%3A%2F%2Fuser%40yap.pr%2F",
        "https%3A%2F%2F%25FF%40yap.pr%2F",
        "https%3A%2F%2Fyap.pr%25FF%40evil.example%2F",
        "https%3A%2F%2Fyap.pr%25FF%3A%25FF%40evil.example%2F",
        "yap.pr%2Fconnect",
        long.as_str(),
    ] {
        assert_uri_invalid(&format!("{base}&cb={callback}"));
    }
    assert_refusal(
        parse(&base.replace("n=t", "n=x")),
        Status::Invalid,
        "unknown network",
    );
}

#[test]
fn an_oversized_dash_key_body_is_refused_before_it_is_decoded() {
    let uri = format!("dash-key:{}?n=t&v=2", "0".repeat(1500));
    assert_refusal(parse(&uri), Status::Invalid, "request is too long");
    assert_refusal(
        parse("dash-key:0?n=t&v=2"),
        Status::Invalid,
        "request is not base58",
    );
    assert_eq!(MAX_KEY_BODY_CHARS, 1407);
}

#[test]
fn an_oversized_dash_key_query_is_refused_before_it_is_read() {
    let v2 = format!("{}&{}", renew_uri(), "x".repeat(4096));
    assert!(!is_connect_link(&v2));
    assert_refusal(parse(&v2), Status::Invalid, "query is too long");

    let at_ceiling = format!("{}&{}", renew_uri(), "x".repeat(4096 - "n=t&v=2&".len()));
    assert!(is_connect_link(&at_ceiling));
    assert!(parse(&at_ceiling).is_ok());
}

#[test]
fn a_dash_key_payload_at_the_ceiling_is_read_and_one_over_is_not() {
    let minimal = [header(), group_binding(), created_at_tlv()].concat();
    let padding = |count: usize| {
        let mut data = Vec::new();
        let mut left = count;
        while left > 0 {
            let chunk = left.min(257);
            data.extend(tlv(0x07, &vec![0; chunk - 2]));
            left -= chunk;
        }
        data
    };
    let uri = |payload: Vec<u8>| format!("dash-key:{}?n=t&v=2", base58(&payload));

    let at = [minimal.clone(), padding(1024 - minimal.len())].concat();
    assert_eq!(at.len(), 1024);
    assert!(parse(&uri(at)).is_ok());
    let over = [minimal.clone(), padding(1025 - minimal.len())].concat();
    assert_refusal(parse(&uri(over)), Status::Invalid, "request is too long");
}

#[test]
fn the_dash_key_query_is_form_decoded_before_it_is_read() {
    let renew = renew_uri();
    let body = renew["dash-key:".len()..].split('?').next().unwrap();
    let parse_query = |query: &str| parse(&format!("dash-key:{body}?{query}"));

    assert_eq!(
        parse_query("%6E=t&%76=%32").unwrap().network,
        NetworkSel::Testnet
    );
    assert_eq!(
        parse_query("?n=t&v=2").unwrap().network,
        NetworkSel::Testnet
    );
    assert_refusal(
        parse_query("n=t&v=2&%76=1"),
        Status::Invalid,
        "query parameter v is repeated",
    );
    assert_eq!(
        parse_query("v=2&\u{301}x=1&n=t").unwrap().network,
        NetworkSel::Testnet
    );

    let callback = |query: &str| parse_query(query).map(|p| p.callback.unwrap().url().to_owned());
    assert_eq!(
        callback("n=t&v=2&cb=https://yap.pr/a-b").unwrap(),
        "https://yap.pr/a-b"
    );
    assert!(parse_query("n=t&v=2&cb=https://yap.pr/a+b").is_err());
    assert!(parse_query("n=t&v=+2").is_err());
    assert_eq!(
        callback("n=t&v=2&cb=https://yap.pr/a%3Fr%3D1").unwrap(),
        "https://yap.pr/a?r=1"
    );
    assert!(parse_query("n=t&v=2&cb=https://yap.pr/a%3Fr%3D1%FF").is_err());
    assert!(parse_query("n=t&v=2&cb=https://yap.pr/a%").is_err());
    for version in ["%", "%4", "", "9999"] {
        assert_refusal(
            parse_query(&format!("n=t&v={version}")),
            Status::Unsupported,
            "URI version (unreadable)",
        );
    }
}

#[test]
fn the_callback_host_must_read_as_written() {
    let base = renew_uri();
    let exact = format!(
        "https://yap.pr/{}",
        "a".repeat(512 - "https://yap.pr/".len())
    );
    let parsed = parse(&format!("{base}&cb={exact}")).unwrap();
    assert_eq!(parsed.callback.unwrap().url(), exact);

    for callback in [
        "https%3A%2F%2Fyap.pr%252F.evil.example%2F",
        "https%3A%2F%2Fyap.pr%2540evil.example%2F",
        "https%3A%2F%2Fyap.pr%3A99999%2F",
        "https%3A%2F%2Fyap.pr%3A99999999999999999999%2F",
        "https%3A%2F%2Fb%C3%BCcher.example%2F",
        "https%3A%2F%2F%5Byap.pr%5D%2F",
        "https%3A%2F%2F%5B%5D%2F",
        "https%3A%2F%2F%5Byap.pr%3Aevil.example%5D%2F",
        "https%3A%2F%2F%5B%3A%3A1%5D%2F",
        "https%3A%2F%2F1.2.3.4%2F",
        "https%3A%2F%2F2130706433%2F",
        "https%3A%2F%2F0x7f.1%2F",
        "https%3A%2F%2Ffoo.123%2F",
        "https%3A%2F%2F1.2.3.0x4%2F",
        "https%3A%2F%2F0x7f000001%2F",
        "https%3A%2F%2FpaypaI.com%2F",
        "https%3A%2F%2FYAP.PR%2F",
        "https%3A%2F%2Fyap..pr%2F",
        "https%3A%2F%2Fyap.pr.%2F",
        "https%3A%2F%2Fyap_pr.example%2F",
    ] {
        assert!(
            parse(&format!("{base}&cb={callback}")).is_err(),
            "{callback}"
        );
    }
    let idn = parse(&format!(
        "{base}&cb=https%3A%2F%2Fxn--bcher-kva.example%3A8443%2F"
    ))
    .unwrap()
    .callback
    .unwrap();
    assert_eq!(idn.port(), Some(8443));
    assert_eq!(idn.host(), "xn--bcher-kva.example");
}

#[test]
fn the_smallest_request_decodes() {
    let request = decode_parts(&[group_binding(), created_at_tlv()]).unwrap();
    assert_eq!(request.auth_bindings.len(), 1);
    assert_eq!(request.auth_bindings[0].expires_at_ms, None);
    assert_eq!(request.identity_hint, None);
    assert_eq!(request.connection_id, None);
    assert!(!request.renew);
}

#[test]
fn unknown_odd_tlvs_are_skipped_and_unknown_even_ones_refuse() {
    let request = decode_parts(&[
        group_binding(),
        created_at_tlv(),
        tlv(0x07, &[1]),
        tlv(0x07, &[]),
        tlv(0x09, &[1, 2, 3]),
    ])
    .unwrap();
    assert_eq!(
        request,
        decode_parts(&[group_binding(), created_at_tlv()]).unwrap()
    );

    assert_unsupported(&[group_binding(), created_at_tlv(), tlv(0x08, &[])]);
    assert_unsupported(&[binding(3, 2, 1), created_at_tlv()]);
    let mut version_3 = header();
    version_3[0] = 0x03;
    assert_eq!(
        refused(AppConnectRequest::decode(&version_3)),
        Refusal::unsupported("request version 3")
    );
}

#[test]
fn a_leading_tlv_type_0_is_invalid_as_on_ios() {
    assert_invalid(&[tlv(0x00, &[]), group_binding(), created_at_tlv()]);
}

#[test]
fn an_unknown_mandatory_part_outranks_a_broken_rule() {
    let bad_level = binding(0, 1, 1);
    let unknown_kind = binding(7, 2, 2);

    assert_invalid(&[bad_level.clone(), created_at_tlv()]);
    assert_invalid(&[group_binding(), encryption(1, 3), created_at_tlv()]);
    assert_unsupported(&[bad_level.clone(), created_at_tlv(), unknown_tlv()]);
    assert_unsupported(&[bad_level.clone(), unknown_kind]);
    assert_unsupported(&[
        group_binding(),
        group_binding(),
        created_at_tlv(),
        unknown_tlv(),
    ]);
    assert_unsupported(&[
        group_binding(),
        encryption(1, 3),
        created_at_tlv(),
        unknown_tlv(),
    ]);
    assert_unsupported(&[
        auth_binding(2, 1, 1, 0, &[]),
        created_at_tlv(),
        unknown_tlv(),
    ]);

    let off_curve = [&[0x02, 0x02][..], &[0xFF; 32], &app_id()].concat();
    let bad_label = [&[3][..], b"a\nb"].concat();
    let refusal = refused(AppConnectRequest::decode(
        &[
            off_curve,
            bad_label,
            group_binding(),
            created_at_tlv(),
            unknown_tlv(),
        ]
        .concat(),
    ));
    assert_eq!(refusal.status, Status::Unsupported);

    let spaced = auth_binding(1, 2, 1, 1, &name("pro file"));
    assert_invalid(&[spaced.clone(), created_at_tlv()]);
    assert_unsupported(&[spaced, created_at_tlv(), unknown_tlv()]);
    assert_invalid(&[
        group_binding(),
        encryption(1, 1),
        encryption(1, 1),
        created_at_tlv(),
    ]);
    assert_unsupported(&[
        group_binding(),
        encryption(1, 1),
        encryption(1, 1),
        created_at_tlv(),
        unknown_tlv(),
    ]);

    let single = |id: u8| binding(0, 2, id);
    let u = unknown_tlv;
    assert_invalid(&[group_binding(), tlv(0x05, &[1, 1]), created_at_tlv(), u()]);
    assert_invalid(&[group_binding(), tlv(0x05, &[2]), created_at_tlv(), u()]);
    assert_invalid(&[group_binding(), tlv(0x06, &u64b(0)), u()]);
    assert_invalid(&[group_binding(), tlv(0x06, &u64b(1 << 53)), u()]);
    let late_expiry = [&[2, 1][..], &group_id(), &u64b(1), &u64b(1 << 53)].concat();
    assert_invalid(&[tlv(0x02, &late_expiry), created_at_tlv(), u()]);
    assert_invalid(&[
        single(1),
        single(2),
        single(3),
        single(4),
        single(5),
        created_at_tlv(),
        u(),
    ]);
    assert_invalid(&[
        group_binding(),
        encryption(1, 1),
        encryption(1, 2),
        encryption(1, 1),
        created_at_tlv(),
        u(),
    ]);
    let short_name = [&[3][..], b"ab"].concat();
    assert_invalid(&[auth_binding(1, 2, 1, 1, &short_name), created_at_tlv(), u()]);
    assert_invalid(&[auth_binding(1, 2, 1, 1, &[1, 0xFF]), created_at_tlv(), u()]);
    assert_invalid(&[
        auth_binding(0, 2, 1, 1, &name("profile")),
        created_at_tlv(),
        u(),
    ]);
    assert_invalid(&[created_at_tlv(), group_binding(), u()]);
}

#[test]
fn text_and_timestamp_edges() {
    let header_with = |label: &[u8]| {
        [
            &[0x02][..],
            &app_eph_pub(),
            &app_id(),
            &[label.len() as u8],
            label,
        ]
        .concat()
    };
    let tail = [group_binding(), created_at_tlv()].concat();

    let bom = [&[0xEF, 0xBB, 0xBF][..], b"Yappr"].concat();
    let with_bom = AppConnectRequest::decode(&[header_with(&bom), tail.clone()].concat()).unwrap();
    assert_eq!(with_bom.label.chars().next(), Some('\u{FEFF}'));
    let bom_name = name_bytes(&[&[0xEF, 0xBB, 0xBF][..], b"profile"].concat());
    assert_invalid(&[auth_binding(1, 2, 1, 1, &bom_name), created_at_tlv()]);
    assert_invalid(&[
        auth_binding(1, 2, 1, 1, &name_bytes(&[0xFF])),
        created_at_tlv(),
    ]);

    let last = decode_parts(&[group_binding(), tlv(0x06, &u64b((1 << 53) - 1))]).unwrap();
    assert_eq!(last.created_at_ms, (1 << 53) - 1);
    let late_expiry = [&[2, 1][..], &group_id(), &u64b(1), &u64b(1 << 53)].concat();
    assert_invalid(&[tlv(0x02, &late_expiry), created_at_tlv()]);
}

#[test]
fn tlv_order_and_count_rules() {
    let hint = tlv(0x01, &[7; 32]);
    let connection = tlv(0x03, &[7; 16]);
    let mode = tlv(0x05, &[1]);

    assert_invalid(&[group_binding(), hint.clone(), created_at_tlv()]);
    assert_invalid(&[created_at_tlv(), group_binding()]);
    assert_invalid(&[hint.clone(), hint, group_binding(), created_at_tlv()]);
    assert_invalid(&[
        group_binding(),
        connection.clone(),
        connection,
        created_at_tlv(),
    ]);
    assert_invalid(&[group_binding(), mode.clone(), mode, created_at_tlv()]);
    assert_invalid(&[group_binding(), created_at_tlv(), created_at_tlv()]);
    assert_invalid(&[group_binding()]);
    assert_invalid(&[created_at_tlv()]);
    assert_invalid(&[
        binding(0, 2, 1),
        binding(0, 2, 2),
        binding(0, 2, 3),
        binding(0, 2, 4),
        binding(0, 2, 5),
        created_at_tlv(),
    ]);
    assert_invalid(&[binding(0, 2, 1), binding(0, 2, 1), created_at_tlv()]);
    assert_invalid(&[
        group_binding(),
        encryption(1, 1),
        encryption(1, 1),
        created_at_tlv(),
    ]);
    assert!(
        decode_parts(&[
            group_binding(),
            encryption(1, 1),
            encryption(1, 2),
            created_at_tlv()
        ])
        .is_ok()
    );
}

#[test]
fn a_third_encryption_binding_is_invalid_as_on_ios() {
    assert_invalid(&[
        group_binding(),
        encryption(1, 1),
        encryption(1, 2),
        encryption(2, 1),
        created_at_tlv(),
    ]);
}

#[test]
fn tlv_value_rules() {
    assert_invalid(&[tlv(0x01, &[7; 31]), group_binding(), created_at_tlv()]);
    assert_invalid(&[group_binding(), tlv(0x03, &[7; 17]), created_at_tlv()]);
    assert_invalid(&[group_binding(), tlv(0x04, &[7; 32]), created_at_tlv()]);
    assert_invalid(&[group_binding(), tlv(0x06, &[1; 7])]);
    assert_invalid(&[group_binding(), encryption(7, 3), created_at_tlv()]);
    assert_invalid(&[group_binding(), tlv(0x05, &[2]), created_at_tlv()]);
    assert_invalid(&[group_binding(), tlv(0x06, &u64b(0))]);
    assert_invalid(&[group_binding(), tlv(0x06, &u64b(1 << 53))]);
    let truncated = created_at_tlv()[..9].to_vec();
    assert_invalid(&[group_binding(), truncated]);
    assert_invalid(&[group_binding(), created_at_tlv(), vec![0x07]]);
}

#[test]
fn auth_binding_rules() {
    assert!(decode_parts(&[binding(2, 2, 1), created_at_tlv()]).is_ok());
    assert_invalid(&[binding(2, 0, 1), created_at_tlv()]);
    assert_invalid(&[binding(2, 3, 1), created_at_tlv()]);
    assert_invalid(&[auth_binding(2, 1, 1, 0, &[]), created_at_tlv()]);
    let unlimited = decode_parts(&[auth_binding(0, 2, 1, 0, &[]), created_at_tlv()]).unwrap();
    assert_eq!(unlimited.auth_bindings[0].budget, None);
    assert_invalid(&[binding(1, 2, 1), created_at_tlv()]);
    assert_invalid(&[auth_binding(1, 2, 1, 1, &[0]), created_at_tlv()]);
    let short_name = [&[3][..], b"ab"].concat();
    assert_invalid(&[auth_binding(1, 2, 1, 1, &short_name), created_at_tlv()]);
    assert_invalid(&[
        auth_binding(1, 2, 1, 1, &name("pro file")),
        created_at_tlv(),
    ]);
    assert_invalid(&[
        auth_binding(1, 2, 1, 1, &name(&"a".repeat(65))),
        created_at_tlv(),
    ]);
    assert!(
        decode_parts(&[
            auth_binding(1, 2, 1, 1, &name(&"a".repeat(64))),
            created_at_tlv()
        ])
        .is_ok()
    );
    assert_invalid(&[auth_binding(0, 2, 1, 1, &name("profile")), created_at_tlv()]);
    let two = decode_parts(&[
        auth_binding(1, 2, 1, 1, &name("a")),
        auth_binding(1, 2, 1, 1, &name("b")),
        created_at_tlv(),
    ])
    .unwrap();
    let names: Vec<_> = two
        .auth_bindings
        .iter()
        .map(|b| match &b.bounds {
            Bounds::DocumentType(_, name) => name.as_str(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(names, ["a", "b"]);
    assert_invalid(&[
        tlv(0x02, &[&[0, 2][..], &[1; 32]].concat()),
        created_at_tlv(),
    ]);
}

#[test]
fn critical_off_a_contract_group_is_invalid_as_on_ios() {
    assert_invalid(&[binding(0, 1, 1), created_at_tlv()]);
    assert_invalid(&[auth_binding(1, 1, 1, 1, &name("profile")), created_at_tlv()]);
    let critical_group = decode_parts(&[binding(2, 1, 1), created_at_tlv()]).unwrap();
    assert_eq!(critical_group.auth_bindings[0].level, Level::Critical);
}

#[test]
fn header_rules() {
    let tail = [group_binding(), created_at_tlv()].concat();
    let key = app_eph_pub();
    let header_with = |key: &[u8], label: &[u8]| {
        [&[0x02][..], key, &app_id(), &[label.len() as u8], label].concat()
    };
    let off_curve = [&[0x02][..], &[0xFF; 32]].concat();
    let empty_header = header_with(&key, &[]);
    let overrunning_label = [&empty_header[..empty_header.len() - 1], &[5], b"ab"].concat();

    assert!(
        AppConnectRequest::decode(&[header_with(&key, "Ünï".as_bytes()), tail.clone()].concat())
            .is_ok()
    );
    let invalid_bodies = [
        vec![],
        empty_header[..empty_header.len() - 1].to_vec(),
        [header_with(&off_curve, &[]), tail.clone()].concat(),
        [header_with(&key, &[0x41; 65]), tail.clone()].concat(),
        overrunning_label,
        [header_with(&key, &[0xC3, 0x28]), tail.clone()].concat(),
        [header_with(&key, &[0xC0, 0xAF]), tail.clone()].concat(),
        [header_with(&key, &[0xED, 0xA0, 0x80]), tail.clone()].concat(),
    ];
    for (index, body) in invalid_bodies.iter().enumerate() {
        let refusal = refused(AppConnectRequest::decode(body));
        assert_eq!(refusal.status, Status::Invalid, "body {index}: {refusal}");
    }
    for forbidden in [
        "a\u{0}b",
        "a\nb",
        "a\u{7F}b",
        "a\u{85}b",
        "a\u{202E}b",
        "a\u{200F}b",
        "a\u{2066}b",
        "a\u{061C}b",
    ] {
        assert_eq!(
            refused(AppConnectRequest::decode(
                &[header_with(&key, forbidden.as_bytes()), tail.clone()].concat()
            )),
            Refusal::invalid("label contains control or bidirectional formatting characters"),
            "{forbidden:?}"
        );
    }
}

#[test]
fn request_age_boundaries() {
    let request = AppConnectRequest::decode(&request_bytes()).unwrap();
    assert_eq!(request.age(CREATED_AT), Age::Fresh);
    assert_eq!(request.age(CREATED_AT + 10 * MINUTE), Age::Fresh);
    assert_eq!(request.age(CREATED_AT + 10 * MINUTE + 1), Age::Stale);
    assert_eq!(request.age(CREATED_AT - 2 * MINUTE), Age::Fresh);
    assert_eq!(request.age(CREATED_AT - 2 * MINUTE - 1), Age::Future);
    assert_eq!(request.age(0), Age::Future);
    assert_eq!(request.age(u64::MAX), Age::Stale);
}

#[test]
fn the_dip_connect_vector_decodes_to_the_vector_body() {
    let parsed = parse(CONNECT_VECTOR_URI).unwrap();
    assert_eq!(base64url_decode(connect_vector_r()), Some(request_bytes()));
    assert_eq!(base64url_encode(&request_bytes()), connect_vector_r());
    assert_eq!(parsed.carrier, Carrier::DashPayConnect);
    assert_eq!(parsed.request, expected_vector_request());
    assert_eq!(parsed.network, NetworkSel::Devnet);
    let callback = parsed.callback.unwrap();
    assert_eq!(callback.url(), CONNECT_VECTOR_CALLBACK);
    assert_eq!(callback.host(), "yap.pr");
    assert_eq!(callback.port(), None);
}

#[test]
fn both_carriers_give_the_same_request() {
    let via_dash_key = parse(&request_uri()).unwrap();
    let via_connect = parse(CONNECT_VECTOR_URI).unwrap();
    assert_eq!(via_connect.request, via_dash_key.request);
    assert_eq!(via_connect.network, via_dash_key.network);
    assert_eq!(via_connect.callback, via_dash_key.callback);
    assert_ne!(via_connect.carrier, via_dash_key.carrier);
}

#[test]
fn the_connect_uri_is_recognised_and_others_are_not() {
    assert!(is_connect_link(CONNECT_VECTOR_URI));
    assert!(is_connect_link(&format!(" {CONNECT_VECTOR_URI}\n")));
    for other in [
        "dashpay://invite?assetlocktx=00",
        "dashpay://connection?v=2",
        "dashpay:connect?v=2",
        "https://connect/",
        "",
    ] {
        assert!(!is_connect_link(other), "{other}");
    }
    assert_ne!(dashpay_host(&request_uri()).as_deref(), Some("connect"));
    assert!(is_connect_link(&request_uri()));
    assert!(is_connect_link("dash-key:abc?n=d&v=%32"));
    assert!(is_connect_link("dash-key:abc?n=d&%76=2"));
    for other in [
        "dash-key:abc?n=d&v=1",
        "dash-st:abc?n=d&v=2",
        "dash-key:abc?n=d&v=22",
        "dash-key:abc?n=d&v",
    ] {
        assert!(!is_connect_link(other), "{other}");
    }
}

#[test]
fn scheme_and_host_are_compared_case_insensitively() {
    let expected = parse(CONNECT_VECTOR_URI).unwrap();
    for prefix in [
        "DASHPAY://CONNECT",
        "DashPay://Connect",
        "dashpay://CONNECT",
        "DASHPAY://connect",
    ] {
        let uri = format!("{prefix}{}", connect_rest());
        assert!(is_connect_link(&uri), "{prefix}");
        assert_eq!(parse(&uri).unwrap(), expected, "{prefix}");
    }
}

#[test]
fn the_path_is_empty_or_a_single_slash() {
    assert!(parse(&format!("dashpay://connect/{}", connect_rest())).is_ok());
    for path in ["//", "/x", "/connect", "/ "] {
        assert_uri_invalid(&format!("dashpay://connect{path}{}", connect_rest()));
    }
}

#[test]
fn userinfo_port_and_fragment_are_refused() {
    let rest = connect_rest();
    for uri in [
        format!("dashpay://user@connect{rest}"),
        format!("dashpay://user:pw@connect{rest}"),
        format!("dashpay://connect:80{rest}"),
        format!("dashpay://connect:{rest}"),
        format!("{CONNECT_VECTOR_URI}#frag"),
        format!("{CONNECT_VECTOR_URI}#"),
        CONNECT_VECTOR_URI.replace("&n=d", "&n=d#x"),
    ] {
        assert_uri_invalid(&uri);
    }
    assert!(is_connect_link(&format!("dashpay://user@connect{rest}")));
    assert!(is_connect_link(&format!("dashpay://connect:80{rest}")));
    assert!(!needs_newer_wallet(&format!("dashpay://connect:80{rest}")));
}

#[test]
fn the_version_must_be_exactly_two() {
    let r = connect_vector_r();
    for query in [
        "n=d", "v=1&n=d", "v=3&n=d", "v=22&n=d", "v=&n=d", "v=02&n=d",
    ] {
        let refusal = refused(parse(&connect_uri(r, query)));
        assert_eq!(refusal.status, Status::Unsupported, "{query}");
    }
    assert_refusal(
        parse(&connect_uri(r, "v=22&n=d")),
        Status::Unsupported,
        "URI version 22",
    );
    assert_refusal(
        parse(&connect_uri(r, "n=d")),
        Status::Unsupported,
        "URI version (none)",
    );
    assert!(parse(&connect_uri(r, "%76=%32&n=d")).is_ok());
}

#[test]
fn the_network_must_be_known() {
    let r = connect_vector_r();
    for query in ["v=2", "v=2&n=", "v=2&n=x", "v=2&n=mt", "v=2&n=D"] {
        assert_refusal(
            parse(&connect_uri(r, query)),
            Status::Invalid,
            "unknown network",
        );
    }
    for (code, network) in [
        ("m", NetworkSel::Mainnet),
        ("t", NetworkSel::Testnet),
        ("d", NetworkSel::Devnet),
    ] {
        let parsed = parse(&connect_uri(r, &format!("v=2&n={code}"))).unwrap();
        assert_eq!(parsed.network, network);
        assert_eq!(network.code(), code);
    }
}

#[test]
fn network_selector_matches_the_wallet_network() {
    assert!(NetworkSel::Mainnet.matches(Network::Mainnet));
    assert!(NetworkSel::Testnet.matches(Network::Testnet));
    assert!(NetworkSel::Devnet.matches(Network::Devnet));
    assert!(NetworkSel::Devnet.matches(Network::Regtest));
    assert!(!NetworkSel::Mainnet.matches(Network::Testnet));
    assert!(!NetworkSel::Testnet.matches(Network::Devnet));
    assert!(!NetworkSel::Devnet.matches(Network::Mainnet));
}

#[test]
fn a_repeated_known_parameter_is_invalid_and_unknown_ones_are_ignored() {
    assert!(parse(&format!("{CONNECT_VECTOR_URI}&x=1&x=2&exp=5")).is_ok());
    let r = connect_vector_r();
    for repeated in [
        "&v=2".to_owned(),
        "&n=d".to_owned(),
        format!("&r={r}"),
        "&cb=https%3A%2F%2Fyap.pr%2F".to_owned(),
        format!("&%72={r}"),
    ] {
        assert_uri_invalid(&format!("{CONNECT_VECTOR_URI}{repeated}"));
    }
    let reordered = format!(
        "dashpay://connect?cb=https%3A%2F%2Fyap.pr%2Fconnect%3Fr%3Dd77dc0da9df83a47755a15c42c5d9db9d7a7fd62&r={r}&n=d&v=2"
    );
    assert_eq!(
        parse(&reordered).unwrap(),
        parse(CONNECT_VECTOR_URI).unwrap()
    );
}

#[test]
fn the_request_is_required() {
    for uri in [
        "dashpay://connect?v=2&n=d",
        "dashpay://connect?v=2&n=d&r=",
        "dashpay://connect",
        "dashpay://connect/",
    ] {
        assert_uri_invalid(uri);
    }
    assert_refusal(
        parse("dashpay://connect?"),
        Status::Unsupported,
        "URI version (none)",
    );
}

#[test]
fn the_request_must_be_canonical_base64url() {
    let r = connect_vector_r();
    assert_eq!(r.len() % 4, 2);
    let cases = [
        ("padding", format!("{r}==")),
        ("plus", format!("+{}", &r[1..])),
        ("plus as space", format!("{}+A", &r[2..])),
        ("slash", format!("/{}", &r[1..])),
        ("outside the alphabet", format!("!{}", &r[1..])),
        ("space", format!("{} ", &r[1..])),
        ("non-ASCII", format!("\u{e9}{}", &r[1..])),
        ("length 1 mod 4", r[..r.len() - 1].to_owned()),
        ("non-zero trailing bits", format!("{}B", &r[..r.len() - 1])),
    ];
    for (case, bad) in cases {
        assert_uri_invalid(&connect_uri(&bad, "v=2&n=d"));
        assert_eq!(base64url_decode(&bad), None, "{case}");
    }
    assert!(base64url_decode(r).is_some());
}

/// RFC 4648 section 5 by bit string, independent of the codec under test.
fn reference_base64url(bytes: &[u8]) -> String {
    let bits: String = bytes.iter().map(|b| format!("{b:08b}")).collect();
    bits.as_bytes()
        .chunks(6)
        .map(|chunk| {
            let mut sextet = String::from_utf8(chunk.to_vec()).unwrap();
            while sextet.len() < 6 {
                sextet.push('0');
            }
            char::from(BASE64URL[usize::from_str_radix(&sextet, 2).unwrap()])
        })
        .collect()
}

#[test]
fn base64url_matches_rfc_4648_and_is_strict() {
    let text = |s: &str| base64url_decode(s).map(|b| String::from_utf8(b).unwrap());
    assert_eq!(base64url_decode(""), Some(vec![]));
    for (encoded, plain) in [
        ("Zg", "f"),
        ("Zm8", "fo"),
        ("Zm9v", "foo"),
        ("Zm9vYg", "foob"),
        ("Zm9vYmE", "fooba"),
        ("Zm9vYmFy", "foobar"),
    ] {
        assert_eq!(text(encoded).as_deref(), Some(plain));
        assert_eq!(base64url_encode(plain.as_bytes()), encoded);
    }
    assert_eq!(base64url_decode("-_8"), Some(vec![0xfb, 0xff]));
    assert_eq!(base64url_decode("-_-_"), Some(vec![0xfb, 0xff, 0xbf]));

    for bad in [
        "Zg==",
        "Zm8=",
        "Z",
        "Zm9vY",
        "Zh",
        "Zm9",
        "Zm9vYh",
        "Zm+/",
        "Zm/+",
        "+_8",
        "Zm 8",
        "Zm8\n",
        "Zm\u{0}8",
        "Z\u{e9}",
        "\u{ff21}\u{ff21}",
    ] {
        assert_eq!(base64url_decode(bad), None, "{bad:?}");
    }
    for length in 0..=40u8 {
        let data: Vec<u8> = (0..length)
            .map(|i| i.wrapping_mul(37).wrapping_add(11))
            .collect();
        let encoded = reference_base64url(&data);
        assert_eq!(base64url_encode(&data), encoded, "length {length}");
        assert_eq!(base64url_decode(&encoded), Some(data), "length {length}");
    }
}

#[test]
fn the_request_limits() {
    let at_the_limit = parse(&connect_uri(&"A".repeat(1366), "v=2&n=d"));
    assert_ne!(
        refused(at_the_limit).detail,
        "request is too long",
        "1,366 characters are 1,024 bytes"
    );
    for count in [1367, 1368, 2000] {
        assert_refusal(
            parse(&connect_uri(&"A".repeat(count), "v=2&n=d")),
            Status::Invalid,
            "request is too long",
        );
    }
    assert_refusal(
        parse(&connect_uri(&"!".repeat(1367), "v=2&n=d")),
        Status::Invalid,
        "request is too long",
    );
}

#[test]
fn the_whole_uri_is_limited_to_4096_bytes_before_decoding() {
    let filler = format!("&x={}", "a".repeat(4096 - CONNECT_VECTOR_URI.len() - 3));
    let at = format!("{CONNECT_VECTOR_URI}{filler}");
    assert_eq!(at.len(), 4096);
    assert!(parse(&at).is_ok());
    assert_refusal(parse(&format!("{at}a")), Status::Invalid, "URI is too long");

    let multibyte = format!("dashpay://connect?v=9&x={}", "\u{20ac}".repeat(1366));
    assert!(multibyte.len() > 4096);
    assert!(multibyte.chars().count() < 4096);
    assert_refusal(parse(&multibyte), Status::Invalid, "URI is too long");
}

#[test]
fn the_return_link_keeps_its_rules() {
    let uri = connect_uri(connect_vector_r(), "v=2&n=d");
    assert_eq!(parse(&uri).unwrap().callback, None);
    for callback in [
        "http%3A%2F%2Fyap.pr%2F",
        "https%3A%2F%2Fuser%40yap.pr%2F",
        "yap.pr%2Fconnect",
        "https%3A%2F%2F1.2.3.4%2F",
        "https%3A%2F%2FYAP.PR%2F",
        "https%3A%2F%2Fyap.pr%3A%2F",
        "https%3A%2F%2F%40yap.pr%2F",
        "https%3Ayap.pr%2F",
        "https%3A%2F%2Fyap.pr%2F%23a%23b",
        "https%3A%2F%2Fyap.pr%2Fa%20b",
    ] {
        assert_uri_invalid(&format!("{uri}&cb={callback}"));
    }
    let with_port = parse(&format!("{uri}&cb=https%3A%2F%2Fyap.pr%3A8443%2Fa")).unwrap();
    assert_eq!(with_port.callback.unwrap().port(), Some(8443));
    for (written, url) in [
        ("https://yap.pr", "https://yap.pr"),
        ("https://yap.pr:0/", "https://yap.pr:0/"),
        ("https://yap.pr:65535/", "https://yap.pr:65535/"),
        ("https://a-b.yap.pr/x?y=1%23z", "https://a-b.yap.pr/x?y=1#z"),
        ("HTTPS://yap.pr/", "HTTPS://yap.pr/"),
    ] {
        let parsed = parse(&format!("{uri}&cb={written}")).unwrap();
        assert_eq!(parsed.callback.unwrap().url(), url);
    }
}

#[test]
fn the_return_link_gets_the_status_appended() {
    let callback = parse(CONNECT_VECTOR_URI).unwrap().callback.unwrap();
    assert_eq!(
        callback.with_status(Status::Ok),
        "https://yap.pr/connect?r=d77dc0da9df83a47755a15c42c5d9db9d7a7fd62&ac=ok"
    );
    let uri = connect_uri(connect_vector_r(), "v=2&n=d");
    let with = |cb: &str, status: Status| {
        parse(&format!("{uri}&cb={cb}"))
            .unwrap()
            .callback
            .unwrap()
            .with_status(status)
    };
    assert_eq!(
        with("https://yap.pr/connect", Status::Declined),
        "https://yap.pr/connect?ac=declined"
    );
    assert_eq!(
        with("https://yap.pr/connect?", Status::Stale),
        "https://yap.pr/connect?ac=stale"
    );
    assert_eq!(
        with(
            "https%3A%2F%2Fyap.pr%2F%3Fac%3Dok%26a%3D1%26%2561c%3Dx",
            Status::Failed
        ),
        "https://yap.pr/?a=1&ac=failed"
    );
    assert_eq!(
        with("https%3A%2F%2Fyap.pr%2F%3Fa%3D1%26%26b%3D2", Status::Ok),
        "https://yap.pr/?a=1&&b=2&ac=ok"
    );
    assert_eq!(
        with("https%3A%2F%2Fyap.pr%2Fx%3Fa%3D1%23top", Status::Invalid),
        "https://yap.pr/x?a=1&ac=invalid#top"
    );
    let statuses = [
        (Status::Ok, "ok"),
        (Status::Declined, "declined"),
        (Status::Stale, "stale"),
        (Status::Unsupported, "unsupported"),
        (Status::Invalid, "invalid"),
        (Status::Failed, "failed"),
    ];
    for (status, text) in statuses {
        assert_eq!(status.as_str(), text);
        assert_eq!(status.to_string(), text);
    }
}

#[test]
fn unknown_verbs_need_a_newer_wallet() {
    for uri in [
        "dashpay://sign?v=2&n=t&exp=1&st=AAAA",
        "DASHPAY://SIGN?v=2",
        "dashpay://sign",
        "dashpay://future/",
        "dashpay://x@sign?v=2",
    ] {
        assert!(needs_newer_wallet(uri), "{uri}");
        assert!(!is_connect_link(uri), "{uri}");
        assert_eq!(classify_link(uri), DeepLink::NeedsNewerWallet, "{uri}");
    }
    let request_uri = request_uri();
    for uri in [
        CONNECT_VECTOR_URI,
        "dashpay://invite?assetlocktx=00",
        "DashPay://Invite?x=1",
        "dashpay://user?id=1",
        "dashpay://",
        "dashpay:sign",
        "dash://sign",
        "https://sign/",
        request_uri.as_str(),
        "dash-st:abc?n=t&v=1",
        "",
    ] {
        assert!(!needs_newer_wallet(uri), "{uri}");
    }
}

#[test]
fn deep_link_routing_takes_connect_and_leaves_invitations_alone() {
    assert_eq!(
        classify_link(CONNECT_VECTOR_URI),
        DeepLink::DashConnect(CONNECT_VECTOR_URI.to_owned())
    );
    assert_eq!(
        classify_link(&format!("  {CONNECT_VECTOR_URI}\n")),
        DeepLink::DashConnect(CONNECT_VECTOR_URI.to_owned())
    );
    assert_eq!(
        classify_link("DASHPAY://CONNECT?v=2"),
        DeepLink::DashConnect("DASHPAY://CONNECT?v=2".to_owned())
    );
    for invite in [
        "dashpay://invite?assetlocktx=00&pk=1",
        "DashPay://INVITE?x=1",
    ] {
        assert!(
            matches!(classify_link(invite), DeepLink::Invitation(_)),
            "{invite}"
        );
    }
    assert_eq!(
        classify_link("dashpay://sign?v=2&n=t"),
        DeepLink::NeedsNewerWallet
    );
    assert_eq!(
        classify_link(&request_uri()),
        DeepLink::DashConnectKey(request_uri())
    );
    assert_eq!(
        classify_link("dash-st:abc?n=t&v=1"),
        DeepLink::DashConnectSt("dash-st:abc?n=t&v=1".to_owned())
    );
    assert!(!matches!(
        classify_link("dash:XyZ?amount=1"),
        DeepLink::DashConnect(_) | DeepLink::NeedsNewerWallet
    ));
    let uphold_cb = format!("{CONNECT_VECTOR_URI}&x=uphold");
    assert_eq!(classify_link(&uphold_cb), DeepLink::DashConnect(uphold_cb));
    assert!(matches!(
        classify_link("dashpay://sign?ret=uphold"),
        DeepLink::Integration(_)
    ));
    let long_invite = format!("dashpay://{}@invite?du=a", "a".repeat(260));
    assert!(needs_newer_wallet(&long_invite));
    assert!(matches!(
        classify_link(&long_invite),
        DeepLink::Invitation(_)
    ));
}

#[test]
fn an_oversized_dashpay_uri_is_classified_from_its_first_scalars() {
    let too_long = Refusal::invalid("URI is too long");
    let connect_with_port = format!("dashpay://connect:{}", "9".repeat(10_000));
    assert!(is_connect_link(&connect_with_port));
    assert!(!needs_newer_wallet(&connect_with_port));
    assert_eq!(refused(parse(&connect_with_port)), too_long);

    let filler = "a".repeat(10_000);
    for uri in [
        format!("dashpay://{filler}"),
        format!("dashpay://connect{filler}"),
        format!("DASHPAY://SIGN{filler}"),
    ] {
        assert!(!is_connect_link(&uri));
        assert!(needs_newer_wallet(&uri));
        assert_eq!(refused(parse(&uri)), too_long);
    }

    let huge = format!("dashpay://{}", "a".repeat(5_000_000));
    assert!(!is_connect_link(&huge));
    assert!(needs_newer_wallet(&huge));
    assert_eq!(refused(parse(&huge)), too_long);

    assert!(is_connect_link(&format!(
        "dashpay://{}@connect?v=2",
        "a".repeat(200)
    )));
}

#[test]
fn not_an_app_connect_uri() {
    assert_refusal(
        parse("dash:XyZ?amount=1"),
        Status::Invalid,
        "not a dashpay://connect URI",
    );
    assert_refusal(parse("dash-key:abc"), Status::Invalid, "URI has no query");
}

#[test]
fn a_refusal_never_carries_the_body() {
    let mut body = request_bytes();
    body[1 + 33 + 32 + 1] = b'\n';
    let r = base64url_encode(&body);
    let refusal = refused(parse(&connect_uri(&r, "v=2&n=d")));
    assert_eq!(refusal.status, Status::Invalid);
    assert!(!refusal.detail.contains(&r));
    assert!(!refusal.detail.contains("Yappr"));
}

#[test]
fn check_supported_refuses_encryption_bindings_off_the_app() {
    let mut request = expected_vector_request();
    assert_eq!(request.check_supported(), Ok(()));
    request.enc_bindings.push(EncBinding {
        contract: app_id(),
        purpose: EncPurpose::Decryption,
    });
    assert_eq!(request.check_supported(), Ok(()));
    request.enc_bindings[1].contract = group_id();
    let reparsed = AppConnectRequest::decode(&request.encode()).unwrap();
    assert_eq!(reparsed, request);
    assert_eq!(
        reparsed.check_supported().unwrap_err().status,
        Status::Unsupported
    );
}

#[test]
fn forge_vectors_parse_with_the_expected_fields() {
    let forge: Value = serde_json::from_str(FORGE_VECTORS).unwrap();
    let id = |key: &str| -> [u8; 32] {
        dashcore::base58::decode(forge["inputs"][key].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap()
    };

    let login = parse(forge["login"]["uri"].as_str().unwrap()).unwrap();
    let login_bytes = hex(forge["login"]["bytes"].as_str().unwrap());
    assert_eq!(login.carrier, Carrier::DashPayConnect);
    assert_eq!(login.network, NetworkSel::Testnet);
    assert_eq!(login.callback, None);
    assert_eq!(login.request.encode(), login_bytes);
    assert_eq!(
        to_hex(&login.request.request_hash()),
        forge["login"]["requestHash"].as_str().unwrap()
    );
    assert_eq!(login.request.label, "Dash Forge");
    assert_eq!(login.request.app_id, id("forgeCore"));
    assert_eq!(login.request.identity_hint, None);
    assert_eq!(
        login.request.auth_bindings,
        [AuthBinding {
            bounds: Bounds::Group(id("forgeGroup")),
            level: Level::High,
            budget: Some(25_000_000_000),
            expires_at_ms: Some(CREATED_AT - 5000 + 180 * 24 * 60 * MINUTE),
        }]
    );
    assert!(login.request.enc_bindings.is_empty());
    assert!(!login.request.renew);
    assert_eq!(login.request.created_at_ms, CREATED_AT);
    assert!(login.request.connection_id.is_some());
    assert_eq!(login.request.check_supported(), Ok(()));

    let renewal = parse(forge["renewal"]["uri"].as_str().unwrap()).unwrap();
    assert_eq!(renewal.network, NetworkSel::Mainnet);
    assert_eq!(
        renewal.request.encode(),
        hex(forge["renewal"]["bytes"].as_str().unwrap())
    );
    assert_eq!(renewal.request.identity_hint, Some(id("alice")));
    assert!(!renewal.request.renew);
    assert_eq!(renewal.request.auth_bindings, login.request.auth_bindings);
    assert_eq!(renewal.request.app_eph_pub, login.request.app_eph_pub);
}

#[test]
fn a_govhub_shaped_request_round_trips_on_mainnet() {
    let forum: [u8; 32] = dashcore::base58::decode("4EcYtwedNjTm4iGmx47GkLX2uNkCUWS4tt1KHhgGkjR3")
        .unwrap()
        .try_into()
        .unwrap();
    let request = AppConnectRequest {
        app_eph_pub: app_eph_pub(),
        app_id: forum,
        label: "govhub".into(),
        identity_hint: None,
        auth_bindings: vec![AuthBinding {
            bounds: Bounds::Contract(forum),
            level: Level::High,
            budget: Some(10_000_000_000),
            expires_at_ms: Some(CREATED_AT + 90 * 24 * 60 * MINUTE),
        }],
        connection_id: Some([0x42; 16]),
        enc_bindings: vec![],
        renew: false,
        created_at_ms: CREATED_AT,
    };
    let uri = format!(
        "dashpay://connect?v=2&n=m&r={}",
        base64url_encode(&request.encode())
    );
    let parsed = parse(&uri).unwrap();
    assert_eq!(parsed.network, NetworkSel::Mainnet);
    assert_eq!(parsed.request, request);
    assert_eq!(parsed.request.check_supported(), Ok(()));
}
