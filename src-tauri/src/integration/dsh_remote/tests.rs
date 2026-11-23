use super::{
    api::{validate_server_url, ServerApi},
    crypto::{builder, prologue, DeviceIdentity, Initiator},
    protocol::*,
    relay::RelayChannel,
    rpc::{CallSemantics, RpcClient},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};
use tokio_tungstenite::{accept_async, tungstenite::Message, WebSocketStream};

fn fixture() -> Value {
    serde_json::from_str::<Value>(include_str!("fixtures/noise-ik.json")).unwrap()["vectors"][0]
        .clone()
}
fn bytes(value: &Value) -> Vec<u8> {
    decode_base64(value.as_str().unwrap()).unwrap()
}

#[test]
fn upstream_noise_ik_golden_vector_matches_snow() {
    let v = fixture();
    let binding = prologue(
        v["prologue"]["connectionId"].as_str().unwrap(),
        v["prologue"]["hostDeviceId"].as_str().unwrap(),
        v["prologue"]["clientDeviceId"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(binding, bytes(&v["prologue"]["bytes"]));
    let private_i = bytes(&v["staticKeys"]["initiator"]["privateKey"]);
    let private_r = bytes(&v["staticKeys"]["responder"]["privateKey"]);
    let public_r = bytes(&v["staticKeys"]["responder"]["publicKey"]);
    let ephemeral_i = bytes(&v["ephemeralPrivateKeys"]["initiator"]);
    let ephemeral_r = bytes(&v["ephemeralPrivateKeys"]["responder"]);
    let mut initiator = builder()
        .unwrap()
        .local_private_key(&private_i)
        .remote_public_key(&public_r)
        .prologue(&binding)
        .fixed_ephemeral_key_for_testing_only(&ephemeral_i)
        .build_initiator()
        .unwrap();
    let mut responder = builder()
        .unwrap()
        .local_private_key(&private_r)
        .prologue(&binding)
        .fixed_ephemeral_key_for_testing_only(&ephemeral_r)
        .build_responder()
        .unwrap();
    let mut output = vec![0; 65535];
    let mut plain = vec![0; 65535];
    let size = initiator
        .write_message(&bytes(&v["handshake"]["message1"]["payload"]), &mut output)
        .unwrap();
    assert_eq!(
        &output[..size],
        bytes(&v["handshake"]["message1"]["message"])
    );
    let size = responder.read_message(&output[..size], &mut plain).unwrap();
    assert_eq!(
        &plain[..size],
        bytes(&v["handshake"]["message1"]["payload"])
    );
    assert_eq!(
        responder.get_remote_static().unwrap(),
        bytes(&v["staticKeys"]["initiator"]["publicKey"])
    );
    let size = responder
        .write_message(&bytes(&v["handshake"]["message2"]["payload"]), &mut output)
        .unwrap();
    assert_eq!(
        &output[..size],
        bytes(&v["handshake"]["message2"]["message"])
    );
    initiator.read_message(&output[..size], &mut plain).unwrap();
    let mut initiator = initiator.into_transport_mode().unwrap();
    let mut responder = responder.into_transport_mode().unwrap();
    assert_eq!(initiator.sending_nonce(), 0);
    assert_eq!(initiator.receiving_nonce(), 0);
    for vector in v["transport"].as_array().unwrap() {
        let (sender, receiver, send_field, receive_field) = if vector["sender"] == "initiator" {
            (
                &mut initiator,
                &mut responder,
                "initiatorSending",
                "responderReceiving",
            )
        } else {
            (
                &mut responder,
                &mut initiator,
                "responderSending",
                "initiatorReceiving",
            )
        };
        let size = sender
            .write_message(&bytes(&vector["plaintext"]), &mut output)
            .unwrap();
        assert_eq!(&output[..size], bytes(&vector["ciphertext"]));
        let plain_size = receiver.read_message(&output[..size], &mut plain).unwrap();
        assert_eq!(&plain[..plain_size], bytes(&vector["plaintext"]));
        assert_eq!(
            sender.sending_nonce().to_string(),
            vector["countersAfter"][send_field].as_str().unwrap()
        );
        assert_eq!(
            receiver.receiving_nonce().to_string(),
            vector["countersAfter"][receive_field].as_str().unwrap()
        );
    }
    let identity = DeviceIdentity::restore(
        "client".into(),
        v["staticKeys"]["initiator"]["privateKey"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(
        identity.public_key,
        v["staticKeys"]["initiator"]["publicKey"]
    );
}

#[test]
fn upstream_control_envelope_and_relay_fixtures() {
    for source in [
        include_str!("fixtures/envelope.json"),
        include_str!("fixtures/relay.json"),
    ] {
        let fixture: Value = serde_json::from_str(source).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let input = serde_json::to_vec(&case["input"]).unwrap();
            assert_eq!(
                Envelope::decode(&input, MAX_RELAY_BYTES).is_ok(),
                case["expect"]["accepted"].as_bool().unwrap(),
                "{}",
                case["name"]
            );
        }
    }
    assert!(strict_json(br#"{"v":1,"\u0076":2}"#).is_err());
    assert!(strict_json(br#"{"nested":{"key":1,"key":2}}"#).is_err());
    assert!(strict_json(&[0xff]).is_err());
    assert!(decode_base64("AQIDBA==").is_err());
    assert!(decode_base64("AQIDBB").is_err());
    assert!(prologue("wrong\0binding", "host", "client").is_err());
}

#[test]
fn fragments_round_trip_and_reject_reordering_duplicates_and_limits() {
    let message = vec![0x41; CHUNK_BYTES * 2 + 123];
    let mut encoder = SecureCodec::default();
    let frames = encoder.encode(&message).unwrap();
    assert_eq!(&frames[0][..5], b"DSHF\x01");
    assert_eq!(&frames[0][5..9], &1u32.to_be_bytes());
    let mut decoder = SecureCodec::default();
    assert!(decoder.decode(&frames[0]).unwrap().is_none());
    assert!(decoder.decode(&frames[1]).unwrap().is_none());
    assert_eq!(decoder.decode(&frames[2]).unwrap().unwrap(), message);
    assert!(SecureCodec::default().decode(&frames[1]).is_err());
    let mut decoder = SecureCodec::default();
    decoder.decode(&frames[0]).unwrap();
    assert!(decoder.decode(&frames[0]).is_err());
    let mut bad = frames[0].clone();
    bad[13..17].copy_from_slice(&((MAX_MESSAGE_BYTES + 1) as u32).to_be_bytes());
    assert!(SecureCodec::default().decode(&bad).is_err());
    assert!(encoder.encode(&vec![0; MAX_MESSAGE_BYTES + 1]).is_err());
    let mut decoder = SecureCodec::default();
    for _ in 0..8 {
        decoder
            .decode(&encoder.encode(&message).unwrap()[0])
            .unwrap();
    }
    assert!(decoder
        .decode(&encoder.encode(&message).unwrap()[0])
        .is_err());
}

#[test]
fn public_http_rejected_and_local_origins_checked_without_dns() {
    for url in [
        "https://example.com",
        "http://127.0.0.1:8000",
        "http://192.168.2.1",
        "http://[::1]",
        "http://[fd00::1]",
    ] {
        assert!(validate_server_url(url).is_ok(), "{url}");
    }
    for url in [
        "http://example.com",
        "http://8.8.8.8",
        "https://user:pass@example.com",
        "https://example.com/path",
        "https://example.com?token=x",
        "file:///tmp/remote",
    ] {
        assert!(validate_server_url(url).is_err(), "{url}");
    }
}

#[test]
fn noise_rejects_wrong_binding_tampering_and_replay() {
    let v = fixture();
    let client = DeviceIdentity::restore(
        "client".into(),
        v["staticKeys"]["initiator"]["privateKey"].as_str().unwrap(),
    )
    .unwrap();
    let host_private = bytes(&v["staticKeys"]["responder"]["privateKey"]);
    let binding = prologue("connection", "host", "client").unwrap();
    let mut client_handshake = Initiator::new(
        &client,
        v["staticKeys"]["responder"]["publicKey"].as_str().unwrap(),
        &binding,
    )
    .unwrap();
    let mut host = builder()
        .unwrap()
        .local_private_key(&host_private)
        .prologue(&binding)
        .build_responder()
        .unwrap();
    let mut buffer = vec![0; 65535];
    host.read_message(&client_handshake.first_message().unwrap(), &mut buffer)
        .unwrap();
    let size = host.write_message(&[], &mut buffer).unwrap();
    let mut client_channel = client_handshake.finish(&buffer[..size]).unwrap();
    let mut host = host.into_transport_mode().unwrap();
    let size = host.write_message(b"secret", &mut buffer).unwrap();
    assert_eq!(
        client_channel.decrypt(0, &buffer[..size]).unwrap(),
        b"secret"
    );
    assert!(client_channel.decrypt(0, &buffer[..size]).is_err());
    let size = host.write_message(b"next", &mut buffer).unwrap();
    buffer[0] ^= 1;
    assert!(client_channel.decrypt(1, &buffer[..size]).is_err());
    let wrong_binding = prologue("different", "host", "client").unwrap();
    let mut client = Initiator::new(
        &client,
        v["staticKeys"]["responder"]["publicKey"].as_str().unwrap(),
        &wrong_binding,
    )
    .unwrap();
    let mut host = builder()
        .unwrap()
        .local_private_key(&host_private)
        .prologue(&binding)
        .build_responder()
        .unwrap();
    assert!(host
        .read_message(&client.first_message().unwrap(), &mut buffer)
        .is_err());
}

async fn http_reply(mut socket: TcpStream, body: Value) -> String {
    let mut request = Vec::new();
    let mut chunk = [0; 1024];
    while !request.windows(4).any(|part| part == b"\r\n\r\n") {
        let size = socket.read(&mut chunk).await.unwrap();
        assert!(size > 0);
        request.extend_from_slice(&chunk[..size]);
        assert!(request.len() < 16384);
    }
    let body = body.to_string();
    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
    socket.write_all(response.as_bytes()).await.unwrap();
    String::from_utf8(request).unwrap()
}
async fn read_ws(socket: &mut WebSocketStream<TcpStream>) -> Envelope {
    let text = socket.next().await.unwrap().unwrap().into_text().unwrap();
    Envelope::decode(text.as_bytes(), MAX_RELAY_BYTES).unwrap()
}
async fn write_ws(socket: &mut WebSocketStream<TcpStream>, kind: &str, payload: Value) {
    let text = String::from_utf8(
        Envelope::new(kind, payload)
            .encode(MAX_RELAY_BYTES)
            .unwrap(),
    )
    .unwrap();
    socket.send(Message::Text(text.into())).await.unwrap();
}
async fn send_remote(
    socket: &mut WebSocketStream<TcpStream>,
    transport: &mut snow::TransportState,
    codec: &mut SecureCodec,
    message: Envelope,
) {
    for chunk in codec
        .encode(&message.encode(MAX_MESSAGE_BYTES).unwrap())
        .unwrap()
    {
        let counter = transport.sending_nonce();
        let mut buffer = vec![0; 65535];
        let size = transport.write_message(&chunk, &mut buffer).unwrap();
        write_ws(socket, "relay", json!({"connectionId":"wire-connection","targetDeviceId":"client","counter":counter,"ciphertext":encode_base64(&buffer[..size])})).await;
    }
}

#[tokio::test]
async fn real_rest_noise_relay_rpc_timeout_never_replays_mutation() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let v = fixture();
    let server_vector = v.clone();
    let server = tokio::spawn(async move {
        let (http, _) = listener.accept().await.unwrap();
        let request = http_reply(http, json!({"deviceId":"host","name":"Fixture Host","role":"host","platform":"test","membershipId":"member","identityKey":server_vector["staticKeys"]["responder"]["publicKey"]})).await;
        assert!(request.starts_with("GET /api/v1/devices/host "));
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(socket).await.unwrap();
        let hello = read_ws(&mut socket).await;
        assert_eq!(hello.kind, "hello");
        assert_eq!(hello.payload["capabilities"], json!(["transport.relay"]));
        write_ws(&mut socket, "hello.ack", json!({"protocol":1,"serverVersion":"fixture","connectionSessionId":"control-session","heartbeatIntervalMs":50,"maxControlFrameBytes":MAX_CONTROL_BYTES,"maxRelayFrameBytes":MAX_RELAY_BYTES,"capabilities":["transport.relay"]})).await;
        assert_eq!(read_ws(&mut socket).await.kind, "connect.request");
        write_ws(
            &mut socket,
            "connect.accepted",
            json!({"connectionId":"wire-connection"}),
        )
        .await;
        let handshake = read_ws(&mut socket).await;
        assert_eq!(handshake.integer("step").unwrap(), 1);
        let private = bytes(&server_vector["staticKeys"]["responder"]["privateKey"]);
        let binding = prologue("wire-connection", "host", "client").unwrap();
        let mut noise = builder()
            .unwrap()
            .local_private_key(&private)
            .prologue(&binding)
            .build_responder()
            .unwrap();
        let mut buffer = vec![0; 65535];
        noise
            .read_message(
                &decode_base64(handshake.string("data").unwrap()).unwrap(),
                &mut buffer,
            )
            .unwrap();
        assert_eq!(
            noise.get_remote_static().unwrap(),
            bytes(&server_vector["staticKeys"]["initiator"]["publicKey"])
        );
        let size = noise.write_message(&[], &mut buffer).unwrap();
        write_ws(&mut socket, "secure.handshake", json!({"connectionId":"wire-connection","targetDeviceId":"client","step":2,"data":encode_base64(&buffer[..size])})).await;
        let mut noise = noise.into_transport_mode().unwrap();
        let mut codec = SecureCodec::default();
        let mut mutation_seen = 0;
        loop {
            let Some(frame) = socket.next().await else {
                break;
            };
            let frame = frame.unwrap();
            if matches!(frame, Message::Close(_)) {
                let _ = socket.flush().await;
                break;
            }
            let frame =
                Envelope::decode(frame.into_text().unwrap().as_bytes(), MAX_RELAY_BYTES).unwrap();
            if frame.kind == "ping" {
                assert!(!frame.string("nonce").unwrap().is_empty());
                write_ws(&mut socket, "pong", frame.payload).await;
                continue;
            }
            assert_eq!(frame.kind, "relay");
            let size = noise
                .read_message(
                    &decode_base64(frame.string("ciphertext").unwrap()).unwrap(),
                    &mut buffer,
                )
                .unwrap();
            let Some(message) = codec.decode(&buffer[..size]).unwrap() else {
                continue;
            };
            let request = Envelope::decode(&message, MAX_MESSAGE_BYTES).unwrap();
            match request.payload["method"].as_str().unwrap() {
                "mutation" => {
                    mutation_seen += 1;
                    assert_eq!(mutation_seen, 1);
                }
                "read" => {
                    // Late completion cannot satisfy the next request.
                    send_remote(
                        &mut socket,
                        &mut noise,
                        &mut codec,
                        Envelope::new(
                            "rpc.response",
                            json!({"requestId":"op-unknown","result":{"late":true}}),
                        ),
                    )
                    .await;
                    send_remote(
                        &mut socket,
                        &mut noise,
                        &mut codec,
                        Envelope::new(
                            "event",
                            json!({"event":"harness.remote.frame","data":{"sourceId":"event-1"}}),
                        ),
                    )
                    .await;
                    send_remote(
                        &mut socket,
                        &mut noise,
                        &mut codec,
                        Envelope::new(
                            "rpc.response",
                            json!({"requestId":request.id,"result":{"text":"x".repeat(100000)}}),
                        ),
                    )
                    .await;
                }
                "bad-route" => {
                    let response =
                        Envelope::new("rpc.response", json!({"requestId":request.id,"result":{}}))
                            .encode(MAX_MESSAGE_BYTES)
                            .unwrap();
                    let counter = noise.sending_nonce();
                    let size = noise.write_message(&response, &mut buffer).unwrap();
                    write_ws(&mut socket, "relay", json!({"connectionId":"wire-connection","targetDeviceId":"different-client","counter":counter,"ciphertext":encode_base64(&buffer[..size])})).await;
                }
                other => panic!("unexpected method: {other}"),
            }
        }
        assert_eq!(mutation_seen, 1);
    });
    let identity = DeviceIdentity::restore(
        "client".into(),
        v["staticKeys"]["initiator"]["privateKey"].as_str().unwrap(),
    )
    .unwrap();
    let api = ServerApi::new(&origin, Duration::from_secs(2)).unwrap();
    let relay = RelayChannel::connect(
        &api,
        &identity,
        "fixture-access-token",
        "host",
        v["staticKeys"]["responder"]["publicKey"].as_str().unwrap(),
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    let client = RpcClient::start(relay);
    let mut events = client.subscribe();
    let failure = client
        .call_with_id(
            "op-unknown",
            "mutation",
            json!({}),
            CallSemantics::Mutation,
            Duration::from_millis(100),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.code, "RPC_TIMEOUT");
    assert!(failure.outcome_unknown);
    let duplicate = client
        .call_with_id(
            "op-unknown",
            "mutation",
            json!({}),
            CallSemantics::Mutation,
            Duration::from_secs(2),
        )
        .await
        .unwrap_err();
    assert_eq!(duplicate.code, "REQUEST_CONFLICT");
    let response = client
        .call("read", json!({}), Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(response["text"].as_str().unwrap().len(), 100000);
    assert_eq!(events.recv().await.unwrap().kind, "event");
    let rejected = client
        .call("bad-route", json!({}), Duration::from_secs(2))
        .await
        .unwrap_err();
    assert_eq!(rejected.code, "SECURE_CHANNEL_FAILED");
    client.close().await;
    assert!(client.is_closed());
    timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn rest_host_discovery_reads_every_page() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = ServerApi::new(
        &format!("http://{}", listener.local_addr().unwrap()),
        Duration::from_secs(2),
    )
    .unwrap();
    let server = tokio::spawn(async move {
        for (id, next) in [("host-1", Some("opaque-next")), ("host-2", None)] {
            let (socket, _) = listener.accept().await.unwrap();
            let request = http_reply(socket, json!({"items":[{"deviceId":id,"name":id,"role":"host","platform":"test","membershipId":"member","online":true}],"nextCursor":next})).await;
            if id == "host-2" {
                assert!(request.contains("cursor=opaque-next"));
            }
        }
    });
    let hosts = api.list_hosts("fixture-access-token").await.unwrap();
    assert_eq!(hosts.len(), 2);
    timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
}

async fn http_reply_status(mut socket: TcpStream, status: &str, body: Value) -> String {
    let mut request = Vec::new();
    let mut chunk = [0; 1024];
    while !request.windows(4).any(|part| part == b"\r\n\r\n") {
        let size = socket.read(&mut chunk).await.unwrap();
        assert!(size > 0);
        request.extend_from_slice(&chunk[..size]);
        assert!(request.len() < 16384);
    }
    let body = body.to_string();
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
    socket.write_all(response.as_bytes()).await.unwrap();
    String::from_utf8(request).unwrap()
}

#[tokio::test]
async fn oauth_browser_sign_in_discovers_starts_polls_and_registers() {
    use super::api::OAuthQrPoll;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let api = ServerApi::new(&origin, Duration::from_secs(2)).unwrap();
    let scan_origin = origin.clone();
    let server = tokio::spawn(async move {
        // Discovery advertises GitHub (QR-capable) and a redirect-only provider.
        let (socket, _) = listener.accept().await.unwrap();
        let request = http_reply(socket, json!({"items":[
            {"id":"github","displayName":"GitHub","startUrl":"/api/v1/auth/oauth/github/start","qrStartUrl":"/api/v1/auth/oauth/qr/start?provider=github"},
            {"id":"google","displayName":"Google","startUrl":"/api/v1/auth/oauth/google/start"}
        ]})).await;
        assert!(request.starts_with("GET /api/v1/auth/oauth/providers "));
        // Start returns a same-origin scan URL and a bounded lifetime.
        let (socket, _) = listener.accept().await.unwrap();
        let request = http_reply(socket, json!({"qrId":"github-qr-session-1234567890","scanUrl":format!("{scan_origin}/api/v1/auth/q/github-qr-session-1234567890"),"expiresIn":300,"provider":"github"})).await;
        assert!(request.starts_with("POST /api/v1/auth/oauth/qr/start?provider=github "));
        // Poll: pending, then complete with the one-time account token.
        let (socket, _) = listener.accept().await.unwrap();
        let request = http_reply(socket, json!({"status":"pending"})).await;
        assert!(request.starts_with("GET /api/v1/auth/oauth/qr/github-qr-session-1234567890 "));
        let (socket, _) = listener.accept().await.unwrap();
        http_reply(
            socket,
            json!({"status":"complete","token":"account-token-fixture-0123456789"}),
        )
        .await;
        // Account profile under the fresh account token.
        let (socket, _) = listener.accept().await.unwrap();
        let request = http_reply(
            socket,
            json!({"account":"reader@example.test","isAdmin":false}),
        )
        .await;
        assert!(request.starts_with("GET /api/v1/auth/me "));
        assert!(request.contains("Bearer account-token-fixture-0123456789"));
        // Device registration exchanges the account token for a device pair.
        let (socket, _) = listener.accept().await.unwrap();
        let request = http_reply(socket, json!({"accessToken":"device-access-fixture","accessTokenExpiresAt":9_000_000_000_000u64,"refreshToken":"device-refresh-fixture","refreshTokenExpiresAt":9_000_000_000_000u64})).await;
        assert!(request.starts_with("POST /api/v1/devices/register "));
        assert!(request.contains("Bearer account-token-fixture-0123456789"));
    });
    let providers = api.oauth_providers().await.unwrap();
    assert_eq!(providers.len(), 2);
    let github = providers.iter().find(|p| p.id == "github").unwrap();
    assert!(providers
        .iter()
        .find(|p| p.id == "google")
        .unwrap()
        .qr_start_url
        .is_none());
    let session = api.start_oauth_qr(github).await.unwrap();
    assert_eq!(session.expires_in, 300);
    assert_eq!(
        api.poll_oauth_qr(&session.qr_id).await.unwrap(),
        OAuthQrPoll::Pending
    );
    let OAuthQrPoll::Complete(token) = api.poll_oauth_qr(&session.qr_id).await.unwrap() else {
        panic!("expected completion");
    };
    assert_eq!(api.account_me(&token).await.unwrap(), "reader@example.test");
    let identity = DeviceIdentity::generate(uuid::Uuid::new_v4().to_string()).unwrap();
    let pair = api
        .register(&identity, "Agentero", "windows", "0.0.0-test", &token)
        .await
        .unwrap();
    assert_eq!(pair.access_token, "device-access-fixture");
    timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn oauth_discovery_falls_back_to_legacy_status_only_on_404() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = ServerApi::new(
        &format!("http://{}", listener.local_addr().unwrap()),
        Duration::from_secs(2),
    )
    .unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let request = http_reply_status(
            socket,
            "404 Not Found",
            json!({"error":{"code":"NOT_FOUND"}}),
        )
        .await;
        assert!(request.starts_with("GET /api/v1/auth/oauth/providers "));
        let (socket, _) = listener.accept().await.unwrap();
        let request = http_reply(socket, json!({"configured":true})).await;
        assert!(request.starts_with("GET /api/v1/auth/oauth/status "));
        let (socket, _) = listener.accept().await.unwrap();
        let request = http_reply_status(socket, "404 Not Found", json!({})).await;
        assert!(request.starts_with("GET /api/v1/auth/oauth/github/status "));
    });
    let providers = api.oauth_providers().await.unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].id, "zhihu");
    assert_eq!(
        providers[0].qr_start_url.as_deref(),
        Some("/api/v1/auth/oauth/qr/start?provider=zhihu")
    );
    timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn oauth_rejects_cross_origin_scan_urls_and_unsafe_entry_paths() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = ServerApi::new(
        &format!("http://{}", listener.local_addr().unwrap()),
        Duration::from_secs(2),
    )
    .unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        http_reply(socket, json!({"items":[
            {"id":"github","displayName":"GitHub","startUrl":"/api/v1/auth/oauth/github/start","qrStartUrl":"/api/v1/auth/oauth/qr/start?provider=github"}
        ]})).await;
        let (socket, _) = listener.accept().await.unwrap();
        http_reply(socket, json!({"qrId":"github-qr-session-1234567890","scanUrl":"https://evil.example/api/v1/auth/q/x","expiresIn":300})).await;
        // A discovery response pointing outside /api/v1/auth/ must be refused.
        let (socket, _) = listener.accept().await.unwrap();
        http_reply(socket, json!({"items":[
            {"id":"github","displayName":"GitHub","startUrl":"https://evil.example/start","qrStartUrl":"/api/v1/auth/oauth/qr/start?provider=github"}
        ]})).await;
        let (socket, _) = listener.accept().await.unwrap();
        http_reply(socket, json!({"items":[
            {"id":"github","displayName":"GitHub","startUrl":"/api/v1/auth/../devices","qrStartUrl":"/api/v1/auth/oauth/qr/start?provider=github"}
        ]})).await;
    });
    let providers = api.oauth_providers().await.unwrap();
    let session = api.start_oauth_qr(&providers[0]).await;
    assert!(session.is_err());
    assert!(api.oauth_providers().await.is_err());
    assert!(api.oauth_providers().await.is_err());
    timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}
