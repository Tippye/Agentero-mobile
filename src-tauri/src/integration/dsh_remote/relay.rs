//! One authenticated relay channel. No automatic reconnect or mutation replay.
use super::{
    api::ServerApi,
    crypto::{prologue, DeviceIdentity, Initiator, SecureChannel},
    protocol::{
        decode_base64, encode_base64, Envelope, SecureCodec, MAX_CONTROL_BYTES, MAX_MESSAGE_BYTES,
        MAX_RELAY_BYTES,
    },
    RemoteError, Result,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::{
    net::TcpStream,
    time::{timeout, Instant},
};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
    MaybeTlsStream, WebSocketStream,
};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct RelayChannel {
    socket: Socket,
    pub connection_id: String,
    local_id: String,
    host_id: String,
    noise: SecureChannel,
    incoming: SecureCodec,
    outgoing: SecureCodec,
    control_limit: usize,
    relay_limit: usize,
    pub(crate) heartbeat: Duration,
    last_received: Instant,
    failed: bool,
}
impl RelayChannel {
    /// `pinned_host_key` must come from a previously approved trust record. Fetch
    /// and compare the current authorized descriptor before opening the channel.
    pub async fn connect(
        api: &ServerApi,
        identity: &DeviceIdentity,
        token: &str,
        host_id: &str,
        pinned_host_key: &str,
        stage_timeout: Duration,
    ) -> Result<Self> {
        if stage_timeout.is_zero() {
            return Err(RemoteError::invalid("Connection timeout must be positive"));
        }
        let host = api.host(token, host_id).await?;
        if host.device.online == Some(false) {
            return Err(RemoteError::new(
                "HOST_OFFLINE",
                "The Host is offline; try again once it reconnects",
            ));
        }
        if host.identity_key != pinned_host_key {
            return Err(RemoteError::new(
                "PEER_IDENTITY_MISMATCH",
                "Host identity changed; explicit trust approval is required",
            ));
        }
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_RELAY_BYTES))
            .max_frame_size(Some(MAX_RELAY_BYTES));
        let (mut socket, _) = timeout(
            stage_timeout,
            connect_async_with_config(api.websocket_url()?.as_str(), Some(config), false),
        )
        .await
        .map_err(|_| RemoteError::new("CONNECTION_FAILED", "Control WebSocket connect timed out"))?
        .map_err(socket_error)?;
        write_frame(&mut socket, &Envelope::new("hello", json!({"role":"client","deviceId":identity.device_id,"accessToken":token,"protocols":[1],"clientVersion":env!("CARGO_PKG_VERSION"),"capabilities":["transport.relay"]})), MAX_CONTROL_BYTES).await?;
        let ack = timeout(
            stage_timeout,
            wait_for(&mut socket, "hello.ack", MAX_CONTROL_BYTES, MAX_RELAY_BYTES),
        )
        .await
        .map_err(|_| RemoteError::new("CONNECTION_FAILED", "Control hello timed out"))??;
        if ack.integer("protocol")? != 1 {
            return Err(RemoteError::new(
                "UNSUPPORTED_VERSION",
                "Server selected an unsupported protocol",
            ));
        }
        ack.string("serverVersion")?;
        ack.string("connectionSessionId")?;
        let control_limit = ack.integer("maxControlFrameBytes")? as usize;
        let relay_limit = ack.integer("maxRelayFrameBytes")? as usize;
        let heartbeat_ms = ack.integer("heartbeatIntervalMs")?;
        if control_limit == 0
            || control_limit > MAX_CONTROL_BYTES
            || relay_limit == 0
            || relay_limit > MAX_RELAY_BYTES
            || !(1..=300_000).contains(&heartbeat_ms)
        {
            return Err(RemoteError::invalid("Invalid hello negotiated limits"));
        }
        if let Some(caps) = ack.payload.get("capabilities") {
            let caps = caps
                .as_array()
                .ok_or_else(|| RemoteError::invalid("Invalid negotiated capabilities"))?;
            if caps.len() != 1 || caps[0].as_str() != Some("transport.relay") {
                return Err(RemoteError::invalid(
                    "Server negotiated capabilities that were not offered",
                ));
            }
        }
        write_frame(
            &mut socket,
            &Envelope::new(
                "connect.request",
                json!({"hostDeviceId":host_id,"preferredTransports":["relay"]}),
            ),
            control_limit,
        )
        .await?;
        let accepted = timeout(
            stage_timeout,
            wait_for(&mut socket, "connect.accepted", control_limit, relay_limit),
        )
        .await
        .map_err(|_| RemoteError::new("CONNECTION_FAILED", "Host authorization timed out"))??;
        // We intentionally offer no relay.attach, so the server must route opaque
        // relay frames over this control socket, including on hosted servers.
        if accepted.payload.get("relayEndpoint").is_some()
            || accepted.payload.get("relayTicket").is_some()
        {
            return Err(RemoteError::invalid(
                "Server selected an unnegotiated relay attachment",
            ));
        }
        let connection_id = accepted.string("connectionId")?.to_owned();
        let binding = prologue(&connection_id, host_id, &identity.device_id)?;
        let mut noise = Initiator::new(identity, pinned_host_key, &binding)?;
        let first = noise.first_message()?;
        write_frame(&mut socket, &Envelope::new("secure.handshake", json!({"connectionId":connection_id,"targetDeviceId":host_id,"step":1,"data":encode_base64(&first)})), control_limit).await?;
        let response = timeout(
            stage_timeout,
            wait_for(&mut socket, "secure.handshake", control_limit, relay_limit),
        )
        .await
        .map_err(|_| RemoteError::new("SECURE_CHANNEL_FAILED", "Noise handshake timed out"))??;
        if response.string("connectionId")? != connection_id
            || response.string("targetDeviceId")? != identity.device_id
            || response.integer("step")? != 2
        {
            return Err(RemoteError::new(
                "SECURE_CHANNEL_FAILED",
                "Noise handshake routing or order mismatch",
            ));
        }
        let channel = noise.finish(&decode_base64(response.string("data")?)?)?;
        Ok(Self {
            socket,
            connection_id,
            local_id: identity.device_id.clone(),
            host_id: host_id.to_owned(),
            noise: channel,
            incoming: SecureCodec::default(),
            outgoing: SecureCodec::default(),
            control_limit,
            relay_limit,
            heartbeat: Duration::from_millis(heartbeat_ms),
            last_received: Instant::now(),
            failed: false,
        })
    }
    pub async fn send(&mut self, message: &Envelope) -> Result<()> {
        if self.failed {
            return Err(RemoteError::new(
                "TRANSPORT_CLOSED",
                "Secure channel is closed",
            ));
        }
        // A cancelled partial send must never reuse the Noise counter state.
        self.failed = true;
        let result = self.send_inner(message).await;
        if result.is_ok() {
            self.failed = false;
        } else {
            let _ = timeout(Duration::from_secs(1), self.socket.close(None)).await;
        }
        result
    }
    async fn send_inner(&mut self, message: &Envelope) -> Result<()> {
        let bytes = message.encode(MAX_MESSAGE_BYTES)?;
        for fragment in self.outgoing.encode(&bytes)? {
            let (counter, ciphertext) = self.noise.encrypt(&fragment)?;
            write_frame(&mut self.socket, &Envelope::new("relay", json!({"connectionId":self.connection_id,"targetDeviceId":self.host_id,"counter":counter,"ciphertext":encode_base64(&ciphertext)})), self.relay_limit).await?;
        }
        Ok(())
    }
    pub async fn receive(&mut self) -> Result<Envelope> {
        if self.failed {
            return Err(RemoteError::new(
                "TRANSPORT_CLOSED",
                "Secure channel is closed",
            ));
        }
        let result = self.receive_inner().await;
        if result.is_err() {
            self.failed = true;
            let _ = timeout(Duration::from_secs(1), self.socket.close(None)).await;
        }
        result
    }
    async fn receive_inner(&mut self) -> Result<Envelope> {
        loop {
            let frame = read_frame(&mut self.socket, self.control_limit, self.relay_limit).await?;
            self.last_received = Instant::now();
            match frame.kind.as_str() {
                "relay" => {
                    self.check_route(&frame)?;
                    let plaintext = self.noise.decrypt(
                        frame.integer("counter")?,
                        &decode_base64(frame.string("ciphertext")?)?,
                    )?;
                    if let Some(message) = self.incoming.decode(&plaintext)? {
                        let message = Envelope::decode(&message, MAX_MESSAGE_BYTES)?;
                        if !matches!(
                            message.kind.as_str(),
                            "rpc.request" | "rpc.response" | "rpc.error" | "event"
                        ) {
                            return Err(RemoteError::invalid("Invalid secure message type"));
                        }
                        return Ok(message);
                    }
                }
                "ping" => {
                    write_frame(
                        &mut self.socket,
                        &Envelope::new("pong", frame.payload),
                        self.control_limit,
                    )
                    .await?
                }
                "pong" => {}
                "error" | "connect.rejected" => return Err(control_error(&frame)),
                // A late Host report cannot select another data plane.
                "transport.selected" => {
                    self.check_route(&frame)?;
                    if frame.string("transport")? != "relay" {
                        return Err(RemoteError::invalid("Conflicting transport selection"));
                    }
                }
                _ => {
                    return Err(RemoteError::invalid(
                        "Unexpected control frame after authentication",
                    ))
                }
            }
        }
    }
    fn check_route(&self, frame: &Envelope) -> Result<()> {
        if frame.string("connectionId")? != self.connection_id
            || frame.string("targetDeviceId")? != self.local_id
        {
            return Err(RemoteError::new(
                "SECURE_CHANNEL_FAILED",
                "Frame belongs to a different connection or device",
            ));
        }
        Ok(())
    }
    pub(crate) async fn heartbeat(&mut self) -> Result<()> {
        if self.last_received.elapsed() > self.heartbeat.saturating_mul(3) {
            return Err(RemoteError::new(
                "CONNECTION_FAILED",
                "Remote heartbeat deadline exceeded",
            ));
        }
        write_frame(
            &mut self.socket,
            &Envelope::new("ping", json!({"nonce":uuid::Uuid::new_v4().to_string()})),
            self.control_limit,
        )
        .await
    }
    pub async fn close(&mut self) -> Result<()> {
        self.failed = true;
        self.socket.close(None).await.map_err(socket_error)
    }
}
async fn wait_for(
    socket: &mut Socket,
    kind: &str,
    control: usize,
    relay: usize,
) -> Result<Envelope> {
    loop {
        let frame = read_frame(socket, control, relay).await?;
        if frame.kind == kind {
            return Ok(frame);
        }
        match frame.kind.as_str() {
            "ping" => write_frame(socket, &Envelope::new("pong", frame.payload), control).await?,
            "pong" => {}
            "error" | "connect.rejected" => return Err(control_error(&frame)),
            _ => {
                return Err(RemoteError::invalid(
                    "Unexpected control frame during connection startup",
                ))
            }
        }
    }
}
async fn read_frame(socket: &mut Socket, control: usize, relay: usize) -> Result<Envelope> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                let frame = Envelope::decode(text.as_bytes(), relay)?;
                if frame.kind != "relay" && text.len() > control {
                    return Err(RemoteError::new(
                        "FRAME_TOO_LARGE",
                        "Control frame exceeds negotiated limit",
                    ));
                }
                return Ok(frame);
            }
            Some(Ok(Message::Ping(payload))) => socket
                .send(Message::Pong(payload))
                .await
                .map_err(socket_error)?,
            Some(Ok(Message::Pong(_))) => {}
            Some(Ok(Message::Close(_))) | None => {
                return Err(RemoteError::new(
                    "TRANSPORT_CLOSED",
                    "Remote WebSocket closed",
                ))
            }
            Some(Ok(_)) => return Err(RemoteError::invalid("Control frames must be text JSON")),
            Some(Err(error)) => return Err(socket_error(error)),
        }
    }
}
async fn write_frame(socket: &mut Socket, frame: &Envelope, limit: usize) -> Result<()> {
    let bytes = frame.encode(limit)?;
    let text =
        String::from_utf8(bytes).map_err(|_| RemoteError::invalid("Invalid outgoing UTF-8"))?;
    timeout(
        Duration::from_secs(10),
        socket.send(Message::Text(text.into())),
    )
    .await
    .map_err(|_| RemoteError::new("CONNECTION_FAILED", "Control WebSocket write timed out"))?
    .map_err(socket_error)
}
fn socket_error(_: tokio_tungstenite::tungstenite::Error) -> RemoteError {
    RemoteError::new("CONNECTION_FAILED", "Remote WebSocket I/O failed")
}
fn control_error(frame: &Envelope) -> RemoteError {
    RemoteError::new(
        frame
            .payload
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("CONNECTION_FAILED"),
        "Remote server rejected or closed the connection",
    )
}
