#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(all(not(target_arch = "wasm32"), feature = "p2p-webrtc-cpp"))]
mod webrtc_cpp;
#[cfg(all(not(target_arch = "wasm32"), feature = "p2p-webrtc-rs"))]
mod webrtc_rs;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::{collections::BTreeMap, time::Duration};

use openmina_core::bug_condition;
use serde::Serialize;
use tokio::sync::Semaphore;

#[cfg(not(target_arch = "wasm32"))]
use tokio::task::spawn_local;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::spawn_local;

use openmina_core::channels::{mpsc, oneshot, Aborted, Aborter};

use crate::identity::{EncryptableType, PublicKey};
use crate::webrtc::{ConnectionAuth, ConnectionAuthEncrypted};
use crate::{
    channels::{ChannelId, ChannelMsg, MsgId},
    connection::outgoing::P2pConnectionOutgoingInitOpts,
    identity::SecretKey,
    webrtc, P2pChannelEvent, P2pConnectionEvent, P2pEvent, PeerId,
};

#[cfg(all(not(target_arch = "wasm32"), feature = "p2p-webrtc-rs"))]
mod imports {
    pub use super::webrtc_rs::{
        build_api, certificate_from_pem_key, webrtc_signal_send, Api, RTCCertificate, RTCChannel,
        RTCConnection, RTCConnectionState, RTCSignalingError,
    };
}
#[cfg(all(not(target_arch = "wasm32"), feature = "p2p-webrtc-cpp"))]
mod imports {
    pub use super::webrtc_cpp::{
        build_api, certificate_from_pem_key, webrtc_signal_send, Api, RTCCertificate, RTCChannel,
        RTCConnection, RTCConnectionState, RTCSignalingError,
    };
}
#[cfg(target_arch = "wasm32")]
mod imports {
    pub use super::web::{
        build_api, certificate_from_pem_key, webrtc_signal_send, Api, RTCCertificate, RTCChannel,
        RTCConnection, RTCConnectionState, RTCSignalingError,
    };
}

use imports::*;
pub use imports::{webrtc_signal_send, RTCSignalingError};

use super::TaskSpawner;

/// 16KB.
const CHUNK_SIZE: usize = 16 * 1024;

pub enum Cmd {
    PeerAdd { args: PeerAddArgs, aborted: Aborted },
}

#[derive(Debug)]
pub enum PeerCmd {
    PeerHttpOfferSend(String, webrtc::Offer),
    AnswerSet(webrtc::Answer),
    ConnectionAuthorizationSend(Option<ConnectionAuthEncrypted>),
    ChannelOpen(ChannelId),
    ChannelSend(MsgId, ChannelMsg),
}

enum PeerCmdInternal {
    ChannelOpened(ChannelId, Result<RTCChannel, Error>),
    ChannelClosed(ChannelId),
}

enum PeerCmdAll {
    External(PeerCmd),
    Internal(PeerCmdInternal),
}

pub struct P2pServiceCtx {
    pub cmd_sender: mpsc::TrackedUnboundedSender<Cmd>,
    pub peers: BTreeMap<PeerId, PeerState>,
}

pub struct PeerAddArgs {
    peer_id: PeerId,
    kind: PeerConnectionKind,
    event_sender: Arc<dyn Fn(P2pEvent) -> Option<()> + Send + Sync + 'static>,
    cmd_receiver: mpsc::TrackedUnboundedReceiver<PeerCmd>,
}

pub enum PeerConnectionKind {
    Outgoing,
    Incoming(Box<webrtc::Offer>),
}

pub struct PeerState {
    pub cmd_sender: mpsc::TrackedUnboundedSender<PeerCmd>,
    pub abort: Aborter,
}

#[derive(thiserror::Error, derive_more::From, Debug)]
pub(super) enum Error {
    #[cfg(all(not(target_arch = "wasm32"), feature = "p2p-webrtc-rs"))]
    #[error("{0}")]
    Rtc(::webrtc::Error),
    #[cfg(all(not(target_arch = "wasm32"), feature = "p2p-webrtc-cpp"))]
    #[error("{0}")]
    Rtc(::datachannel::Error),
    #[cfg(target_arch = "wasm32")]
    #[error("js error: {0:?}")]
    RtcJs(String),
    #[error("signaling error: {0}")]
    Signaling(RTCSignalingError),
    #[error("unexpected cmd received")]
    UnexpectedCmd,
    #[from(ignore)]
    #[error("channel closed")]
    ChannelClosed,
}

#[cfg(target_arch = "wasm32")]
impl From<wasm_bindgen::JsValue> for Error {
    fn from(value: wasm_bindgen::JsValue) -> Self {
        Error::RtcJs(format!("{value:?}"))
    }
}

pub type OnConnectionStateChangeHdlrFn = Box<
    dyn (FnMut(RTCConnectionState) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>>)
        + Send
        + Sync,
>;

pub struct RTCConfig {
    pub ice_servers: RTCConfigIceServers,
    pub certificate: RTCCertificate,
    pub seed: [u8; 32],
}

#[derive(Serialize)]
pub struct RTCConfigIceServers(Vec<RTCConfigIceServer>);
#[derive(Serialize)]
pub struct RTCConfigIceServer {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

#[derive(Serialize)]
pub struct RTCChannelConfig {
    pub label: &'static str,
    pub negotiated: Option<u16>,
}

impl Default for RTCConfigIceServers {
    fn default() -> Self {
        Self(vec![
            RTCConfigIceServer {
                urls: vec!["stun:65.109.110.75:3478".to_owned()],
                username: Some("openmina".to_owned()),
                credential: Some("webrtc".to_owned()),
            },
            RTCConfigIceServer {
                urls: vec!["stun:176.9.147.28:3478".to_owned()],
                username: None,
                credential: None,
            },
            RTCConfigIceServer {
                urls: vec![
                    "stun:stun.l.google.com:19302".to_owned(),
                    "stun:stun1.l.google.com:19302".to_owned(),
                    "stun:stun2.l.google.com:19302".to_owned(),
                    "stun:stun3.l.google.com:19302".to_owned(),
                    "stun:stun4.l.google.com:19302".to_owned(),
                ],
                username: None,
                credential: None,
            },
        ])
    }
}

impl std::ops::Deref for RTCConfigIceServers {
    type Target = Vec<RTCConfigIceServer>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(not(all(not(target_arch = "wasm32"), feature = "p2p-webrtc-cpp")))]
impl Drop for RTCConnection {
    fn drop(&mut self) {
        if self.is_main() {
            let cloned = self.clone();
            spawn_local(async move {
                let _ = cloned.close().await;
            });
        }
    }
}

async fn sleep(dur: Duration) {
    #[cfg(not(target_arch = "wasm32"))]
    let fut = tokio::time::sleep(dur);
    #[cfg(target_arch = "wasm32")]
    let fut = gloo_timers::future::TimeoutFuture::new(dur.as_millis() as u32);
    fut.await
}

async fn wait_for_ice_gathering_complete(pc: &mut RTCConnection) {
    let timeout = sleep(Duration::from_secs(3));

    tokio::select! {
        _ = timeout => {}
        _ = pc.wait_for_ice_gathering_complete() => {}
    }
}

async fn peer_start(
    api: Api,
    args: PeerAddArgs,
    abort: Aborted,
    closed: mpsc::Sender<()>,
    certificate: RTCCertificate,
    rng_seed: [u8; 32],
) {
    let PeerAddArgs {
        peer_id,
        kind,
        event_sender,
        mut cmd_receiver,
    } = args;
    let is_outgoing = matches!(kind, PeerConnectionKind::Outgoing);

    let config = RTCConfig {
        ice_servers: Default::default(),
        certificate,
        seed: rng_seed,
    };
    let fut = async {
        let mut pc = RTCConnection::create(&api, config).await?;
        let main_channel = pc
            .channel_create(RTCChannelConfig {
                label: "",
                negotiated: Some(0),
            })
            .await?;

        let offer = match kind {
            PeerConnectionKind::Incoming(offer) => (*offer).try_into()?,
            PeerConnectionKind::Outgoing => pc.offer_create().await?,
        };

        if is_outgoing {
            pc.local_desc_set(offer).await?;
            wait_for_ice_gathering_complete(&mut pc).await;
        } else {
            pc.remote_desc_set(offer).await?;
        }

        Result::<_, Error>::Ok((pc, main_channel))
    };

    #[allow(unused_mut)]
    let (mut pc, mut main_channel) = match fut.await {
        Ok(v) => v,
        Err(err) => {
            event_sender(P2pConnectionEvent::OfferSdpReady(peer_id, Err(err.to_string())).into());
            return;
        }
    };

    let (main_channel_open_tx, main_channel_open) = oneshot::channel::<()>();
    let mut main_channel_open_tx = Some(main_channel_open_tx);
    main_channel.on_open(move || {
        if let Some(tx) = main_channel_open_tx.take() {
            let _ = tx.send(());
        }
        std::future::ready(())
    });

    let answer = if is_outgoing {
        let answer_fut = async {
            let sdp = pc.local_sdp().await.unwrap();
            event_sender(P2pConnectionEvent::OfferSdpReady(peer_id, Ok(sdp)).into())
                .ok_or(Error::ChannelClosed)?;
            match cmd_receiver.recv().await.ok_or(Error::ChannelClosed)?.0 {
                PeerCmd::PeerHttpOfferSend(url, offer) => {
                    let answer = webrtc_signal_send(&url, offer).await?;
                    event_sender(P2pConnectionEvent::AnswerReceived(peer_id, answer).into())
                        .ok_or(Error::ChannelClosed)?;

                    if let PeerCmd::AnswerSet(v) =
                        cmd_receiver.recv().await.ok_or(Error::ChannelClosed)?.0
                    {
                        return Ok(v);
                    }
                }
                PeerCmd::AnswerSet(v) => return Ok(v),
                _cmd => {
                    return Err(Error::UnexpectedCmd);
                }
            }
            Err(Error::ChannelClosed)
        };
        answer_fut.await.and_then(|v| Ok(v.try_into()?))
    } else {
        pc.answer_create().await.map_err(Error::from)
    };
    let Ok(answer) = answer else {
        return;
    };

    if is_outgoing {
        if let Err(err) = pc.remote_desc_set(answer).await {
            let err = Error::from(err).to_string();
            let _ = event_sender(P2pConnectionEvent::Finalized(peer_id, Err(err)).into());
        }
    } else {
        let fut = async {
            pc.local_desc_set(answer).await?;
            wait_for_ice_gathering_complete(&mut pc).await;
            Ok(pc.local_sdp().await.unwrap())
        };
        let res = fut.await.map_err(|err: Error| err.to_string());
        let is_err = res.is_err();
        let is_err = is_err
            || event_sender(P2pConnectionEvent::AnswerSdpReady(peer_id, res).into()).is_none();
        if is_err {
            return;
        }
    }

    let (connected_tx, connected) = oneshot::channel();
    if matches!(pc.connection_state(), RTCConnectionState::Connected) {
        connected_tx.send(Ok(())).unwrap();
    } else {
        let mut connected_tx = Some(connected_tx);
        pc.on_connection_state_change(Box::new(move |state| {
            match state {
                RTCConnectionState::Connected => {
                    if let Some(connected_tx) = connected_tx.take() {
                        let _ = connected_tx.send(Ok(()));
                    }
                }
                RTCConnectionState::Disconnected | RTCConnectionState::Closed => {
                    if let Some(connected_tx) = connected_tx.take() {
                        let _ = connected_tx.send(Err("disconnected"));
                    } else {
                        let _ = closed.try_send(());
                    }
                }
                _ => {}
            }
            Box::pin(std::future::ready(()))
        }));
    }
    match connected
        .await
        .map_err(|_| Error::ChannelClosed.to_string())
        .and_then(|res| res.map_err(|v| v.to_string()))
    {
        Ok(_) => {}
        Err(err) => {
            let _ = event_sender(P2pConnectionEvent::Finalized(peer_id, Err(err)).into());
            return;
        }
    }

    // Exchange encrypted connection authorization messsages. Makes sure
    // there is a link between peer identity and connection.
    let (remote_auth_tx, remote_auth_rx) = oneshot::channel::<ConnectionAuthEncrypted>();
    let mut remote_auth_tx = Some(remote_auth_tx);
    main_channel.on_message(move |data| {
        if let Some(tx) = remote_auth_tx.take() {
            if let Ok(auth) = data.try_into() {
                let _ = tx.send(auth);
            }
        }
        #[cfg(not(all(not(target_arch = "wasm32"), feature = "p2p-webrtc-cpp")))]
        std::future::ready(())
    });
    let msg = match cmd_receiver.recv().await {
        None => return,
        Some(msg) => msg,
    };
    match msg.0 {
        PeerCmd::ConnectionAuthorizationSend(None) => {
            // eprintln!("PeerCmd::ConnectionAuthorizationSend(None)");
            return;
        }
        PeerCmd::ConnectionAuthorizationSend(Some(auth)) => {
            let _ = main_channel_open.await;

            // Add a delay for sending messages after channel
            // was opened. Some initial messages get lost otherwise.
            // TODO(binier): find deeper cause and fix it.
            sleep(Duration::from_secs(1)).await;
            let _ = main_channel
                .send(&bytes::Bytes::copy_from_slice(auth.as_ref()))
                .await;

            let res = match remote_auth_rx.await {
                Err(_) => Err("didn't receive connection authentication message".to_owned()),
                Ok(remote_auth) => Ok(remote_auth),
            };
            let is_err = res.is_err();
            let _ = event_sender(P2pConnectionEvent::Finalized(peer_id, res).into());
            if is_err {
                return;
            }
        }
        cmd => {
            bug_condition!("unexpected peer cmd! Expected `PeerCmd::ConnectionAuthorizationSend`. received: {cmd:?}");
            return;
        }
    }

    let _ = main_channel.close().await;

    peer_loop(peer_id, event_sender, cmd_receiver, pc, abort).await
}

struct Channel {
    id: ChannelId,
    msg_sender: ChannelMsgSender,
}

type ChannelMsgSender = mpsc::UnboundedSender<(MsgId, Vec<u8>, Option<mpsc::Tracker>)>;

struct MsgBuffer {
    buf: Vec<u8>,
}

impl MsgBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
        }
    }

    fn encode(&mut self, msg: &ChannelMsg) -> Result<Vec<u8>, std::io::Error> {
        msg.encode(&mut self.buf)?;
        let len_encoded = (self.buf.len() as u32).to_be_bytes();
        let encoded = len_encoded
            .into_iter()
            .chain(self.buf.iter().cloned())
            .collect();
        self.buf.clear();
        Ok(encoded)
    }
}

struct Channels {
    list: Vec<Channel>,
}

impl Channels {
    fn new() -> Self {
        Self {
            list: Vec::with_capacity(32),
        }
    }

    fn get_msg_sender(&self, id: ChannelId) -> Option<&ChannelMsgSender> {
        self.list.iter().find(|c| c.id == id).map(|c| &c.msg_sender)
    }

    fn add(&mut self, id: ChannelId, msg_sender: ChannelMsgSender) {
        self.list.push(Channel { id, msg_sender });
    }

    fn remove(&mut self, id: ChannelId) -> bool {
        match self.list.iter().position(|c| c.id == id) {
            None => false,
            Some(index) => {
                self.list.remove(index);
                true
            }
        }
    }
}

// TODO(binier): remove unwraps
#[allow(unused_mut)]
async fn peer_loop(
    peer_id: PeerId,
    event_sender: Arc<dyn Fn(P2pEvent) -> Option<()> + Send + Sync + 'static>,
    mut cmd_receiver: mpsc::TrackedUnboundedReceiver<PeerCmd>,
    mut pc: RTCConnection,
    aborted: Aborted,
) {
    // TODO(binier): maybe use small_vec (stack allocated) or something like that.
    let mut channels = Channels::new();
    let mut msg_buf = MsgBuffer::new(64 * 1024);

    let (internal_cmd_sender, mut internal_cmd_receiver) =
        mpsc::unbounded_channel::<PeerCmdInternal>();

    while matches!(pc.connection_state(), RTCConnectionState::Connected) {
        let (cmd, _tracker) = tokio::select! {
            cmd = cmd_receiver.recv() => match cmd {
                None => return,
                Some(cmd) => (PeerCmdAll::External(cmd.0), Some(cmd.1)),
            },
            cmd = internal_cmd_receiver.recv() => match cmd {
                None => return,
                Some(cmd) => (PeerCmdAll::Internal(cmd), None),
            },
        };
        match cmd {
            PeerCmdAll::External(
                PeerCmd::PeerHttpOfferSend(..)
                | PeerCmd::AnswerSet(_)
                | PeerCmd::ConnectionAuthorizationSend(_),
            ) => {
                bug_condition!("unexpected peer cmd");
            }
            PeerCmdAll::External(PeerCmd::ChannelOpen(id)) => {
                let chan = pc
                    .channel_create(RTCChannelConfig {
                        label: id.name(),
                        negotiated: Some(id.to_u16()),
                    })
                    .await;
                let internal_cmd_sender = internal_cmd_sender.clone();
                let fut = async move {
                    let internal_cmd_sender_clone = internal_cmd_sender.clone();
                    let result = async move {
                        let chan = chan?;

                        let (done_tx, mut done_rx) = mpsc::channel::<Result<(), Error>>(1);

                        let done_tx_clone = done_tx.clone();
                        chan.on_open(move || {
                            let _ = done_tx_clone.try_send(Ok(()));
                            std::future::ready(())
                        });

                        let done_tx_clone = done_tx.clone();
                        let internal_cmd_sender = internal_cmd_sender_clone.clone();
                        chan.on_error(move |err| {
                            if done_tx_clone.try_send(Err(err.into())).is_err() {
                                let _ =
                                    internal_cmd_sender.send(PeerCmdInternal::ChannelClosed(id));
                            }
                            std::future::ready(())
                        });

                        let done_tx_clone = done_tx.clone();
                        let internal_cmd_sender = internal_cmd_sender_clone.clone();
                        chan.on_close(move || {
                            if done_tx_clone.try_send(Err(Error::ChannelClosed)).is_err() {
                                let _ =
                                    internal_cmd_sender.send(PeerCmdInternal::ChannelClosed(id));
                            }
                            std::future::ready(())
                        });

                        done_rx.recv().await.ok_or(Error::ChannelClosed)??;

                        Ok(chan)
                    };

                    let _ =
                        internal_cmd_sender.send(PeerCmdInternal::ChannelOpened(id, result.await));
                };
                let mut aborted = aborted.clone();
                spawn_local(async move {
                    tokio::select! {
                        _ = aborted.wait() => {}
                        _ = fut => {}
                    }
                });
            }
            PeerCmdAll::External(PeerCmd::ChannelSend(msg_id, msg)) => {
                let id = msg.channel_id();
                let err = match channels.get_msg_sender(id) {
                    Some(msg_sender) => match msg_buf.encode(&msg) {
                        Ok(encoded) => match msg_sender.send((msg_id, encoded, _tracker)) {
                            Ok(_) => None,
                            Err(_) => Some("ChannelMsgMpscSendFailed".to_owned()),
                        },
                        Err(err) => Some(err.to_string()),
                    },
                    None => Some("ChannelNotOpen".to_owned()),
                };
                if let Some(err) = err {
                    let _ =
                        event_sender(P2pChannelEvent::Sent(peer_id, id, msg_id, Err(err)).into());
                }
            }
            PeerCmdAll::Internal(PeerCmdInternal::ChannelOpened(chan_id, result)) => {
                let (sender_tx, mut sender_rx) = mpsc::unbounded_channel();
                let (chan, res) = match result {
                    Ok(chan) => {
                        channels.add(chan_id, sender_tx);
                        (Some(chan), Ok(()))
                    }
                    Err(err) => (None, Err(err.to_string())),
                };

                #[allow(unused_mut)]
                if let Some(mut chan) = chan {
                    fn process_msg(
                        chan_id: ChannelId,
                        buf: &mut Vec<u8>,
                        len: &mut u32,
                        msg: &mut &[u8],
                    ) -> Result<Option<ChannelMsg>, String> {
                        let len = if buf.is_empty() {
                            if msg.len() < 4 {
                                return Err("WebRTCMessageTooSmall".to_owned());
                            } else {
                                *len = u32::from_be_bytes(
                                    msg[..4].try_into().expect("Size checked above"),
                                );
                                *msg = &msg[4..];
                                let len = *len as usize;
                                if len > chan_id.max_msg_size() {
                                    return Err(format!(
                                        "ChannelMsgLenOverLimit; len: {}, limit: {}",
                                        len,
                                        chan_id.max_msg_size()
                                    ));
                                }
                                len
                            }
                        } else {
                            *len as usize
                        };
                        let bytes_left = len - buf.len();

                        if bytes_left > msg.len() {
                            buf.extend_from_slice(msg);
                            *msg = &[];
                            return Ok(None);
                        }

                        buf.extend_from_slice(&msg[..bytes_left]);
                        *msg = &msg[bytes_left..];
                        let msg = ChannelMsg::decode(&mut &buf[..], chan_id)
                            .map_err(|err| err.to_string())?;
                        buf.clear();
                        Ok(Some(msg))
                    }

                    let mut len = 0;
                    let mut buf = Vec::new();
                    let event_sender_clone = event_sender.clone();

                    chan.on_message(move |mut data| {
                        while !data.is_empty() {
                            let res = match process_msg(chan_id, &mut buf, &mut len, &mut data) {
                                Ok(None) => continue,
                                Ok(Some(msg)) => Ok(msg),
                                Err(err) => Err(err),
                            };
                            let _ =
                                event_sender_clone(P2pChannelEvent::Received(peer_id, res).into());
                        }
                        #[cfg(not(all(not(target_arch = "wasm32"), feature = "p2p-webrtc-cpp")))]
                        std::future::ready(())
                    });

                    let event_sender = event_sender.clone();
                    let fut = async move {
                        // Add a delay for sending messages after channel
                        // was opened. Some initial messages get lost otherwise.
                        // TODO(binier): find deeper cause and fix it.
                        sleep(Duration::from_secs(3)).await;

                        while let Some((msg_id, encoded, _tracker)) = sender_rx.recv().await {
                            let encoded = bytes::Bytes::from(encoded);
                            let mut chunks =
                                encoded.chunks(CHUNK_SIZE).map(|b| encoded.slice_ref(b));
                            let result = loop {
                                let Some(chunk) = chunks.next() else {
                                    break Ok(());
                                };
                                if let Err(err) = chan
                                    .send(&chunk)
                                    .await
                                    .map_err(|e| format!("{e:?}"))
                                    .and_then(|n| match n == chunk.len() {
                                        false => Err("NotAllBytesWritten".to_owned()),
                                        true => Ok(()),
                                    })
                                {
                                    break Err(err);
                                }
                            };

                            let _ = event_sender(
                                P2pChannelEvent::Sent(peer_id, chan_id, msg_id, result).into(),
                            );
                        }
                    };

                    let mut aborted = aborted.clone();
                    spawn_local(async move {
                        tokio::select! {
                            _ = aborted.wait() => {}
                            _ = fut => {}
                        }
                    });
                }

                let _ = event_sender(P2pChannelEvent::Opened(peer_id, chan_id, res).into());
            }
            PeerCmdAll::Internal(PeerCmdInternal::ChannelClosed(id)) => {
                channels.remove(id);
                let _ = event_sender(P2pChannelEvent::Closed(peer_id, id).into());
            }
        }
    }
}

pub trait P2pServiceWebrtc: redux::Service {
    type Event: From<P2pEvent> + Send + Sync + 'static;

    fn random_pick(
        &mut self,
        list: &[P2pConnectionOutgoingInitOpts],
    ) -> Option<P2pConnectionOutgoingInitOpts>;

    fn event_sender(&self) -> &mpsc::UnboundedSender<Self::Event>;

    fn cmd_sender(&self) -> &mpsc::TrackedUnboundedSender<Cmd>;

    fn peers(&mut self) -> &mut BTreeMap<PeerId, PeerState>;

    fn init<S: TaskSpawner>(
        secret_key: SecretKey,
        spawner: S,
        rng_seed: [u8; 32],
    ) -> P2pServiceCtx {
        const MAX_PEERS: usize = 500;
        let (cmd_sender, mut cmd_receiver) = mpsc::tracked_unbounded_channel();

        let certificate = certificate_from_pem_key(secret_key.to_pem().as_str());

        spawner.spawn_main("webrtc", async move {
            #[allow(clippy::all)]
            let api = build_api();
            let conn_permits = Arc::new(Semaphore::const_new(MAX_PEERS));
            while let Some(cmd) = cmd_receiver.recv().await {
                match cmd.0 {
                    Cmd::PeerAdd { args, aborted } => {
                        #[allow(clippy::all)]
                        let api = api.clone();
                        let conn_permits = conn_permits.clone();
                        let peer_id = args.peer_id;
                        let event_sender = args.event_sender.clone();
                        let certificate = certificate.clone();
                        spawn_local(async move {
                            let Ok(_permit) = conn_permits.try_acquire() else {
                                // state machine shouldn't allow this to happen.
                                bug_condition!("P2P WebRTC Semaphore acquisition failed!");
                                return;
                            };
                            let (closed_tx, mut closed) = mpsc::channel(1);
                            let event_sender_clone = event_sender.clone();
                            spawn_local(async move {
                                // to avoid sending closed multiple times
                                let _ = closed.recv().await;
                                event_sender_clone(P2pConnectionEvent::Closed(peer_id).into());
                            });
                            tokio::select! {
                                _ = peer_start(api, args, aborted.clone(), closed_tx.clone(), certificate, rng_seed) => {}
                                _ = aborted.wait() => {
                                }
                            }

                            // delay dropping permit to give some time for cleanup.
                            sleep(Duration::from_millis(100)).await;
                            let _ = closed_tx.send(()).await;
                        });
                    }
                }
            }
        });

        P2pServiceCtx {
            cmd_sender,
            peers: Default::default(),
        }
    }

    fn outgoing_init(&mut self, peer_id: PeerId) {
        let (peer_cmd_sender, peer_cmd_receiver) = mpsc::tracked_unbounded_channel();
        let aborter = Aborter::default();
        let aborted = aborter.aborted();

        self.peers().insert(
            peer_id,
            PeerState {
                cmd_sender: peer_cmd_sender,
                abort: aborter,
            },
        );
        let event_sender = self.event_sender().clone();
        let event_sender =
            Arc::new(move |p2p_event: P2pEvent| event_sender.send(p2p_event.into()).ok());
        let _ = self.cmd_sender().tracked_send(Cmd::PeerAdd {
            args: PeerAddArgs {
                peer_id,
                kind: PeerConnectionKind::Outgoing,
                event_sender,
                cmd_receiver: peer_cmd_receiver,
            },
            aborted,
        });
    }

    fn incoming_init(&mut self, peer_id: PeerId, offer: webrtc::Offer) {
        let (peer_cmd_sender, peer_cmd_receiver) = mpsc::tracked_unbounded_channel();
        let aborter = Aborter::default();
        let aborted = aborter.aborted();

        self.peers().insert(
            peer_id,
            PeerState {
                cmd_sender: peer_cmd_sender,
                abort: aborter,
            },
        );
        let event_sender = self.event_sender().clone();
        let event_sender =
            Arc::new(move |p2p_event: P2pEvent| event_sender.send(p2p_event.into()).ok());
        let _ = self.cmd_sender().tracked_send(Cmd::PeerAdd {
            args: PeerAddArgs {
                peer_id,
                kind: PeerConnectionKind::Incoming(Box::new(offer)),
                event_sender,
                cmd_receiver: peer_cmd_receiver,
            },
            aborted,
        });
    }

    fn set_answer(&mut self, peer_id: PeerId, answer: webrtc::Answer) {
        if let Some(peer) = self.peers().get(&peer_id) {
            let _ = peer.cmd_sender.tracked_send(PeerCmd::AnswerSet(answer));
        }
    }

    fn http_signaling_request(&mut self, url: String, offer: webrtc::Offer) {
        if let Some(peer) = self.peers().get(&offer.target_peer_id) {
            let _ = peer
                .cmd_sender
                .tracked_send(PeerCmd::PeerHttpOfferSend(url, offer));
        }
    }

    fn disconnect(&mut self, peer_id: PeerId) -> bool {
        // TODO(binier): improve
        // By removing the peer, `abort` gets dropped which will
        // cause `peer_loop` to end.
        if let Some(_peer) = self.peers().remove(&peer_id) {
            // if peer.abort.receiver_count() > 0 {
            //     // peer disconnection not yet finished
            //     return false;
            // }
        } else {
            openmina_core::error!(openmina_core::log::system_time(); "`disconnect` shouldn't be used for libp2p peers");
        }
        true
    }

    fn channel_open(&mut self, peer_id: PeerId, id: ChannelId) {
        if let Some(peer) = self.peers().get(&peer_id) {
            let _ = peer.cmd_sender.tracked_send(PeerCmd::ChannelOpen(id));
        }
    }

    fn channel_send(&mut self, peer_id: PeerId, msg_id: MsgId, msg: ChannelMsg) {
        if let Some(peer) = self.peers().get(&peer_id) {
            let _ = peer
                .cmd_sender
                .tracked_send(PeerCmd::ChannelSend(msg_id, msg));
        }
    }

    fn encrypt<T: EncryptableType>(
        &mut self,
        other_pk: &PublicKey,
        message: &T,
    ) -> Result<T::Encrypted, Box<dyn std::error::Error>>;

    fn decrypt<T: EncryptableType>(
        &mut self,
        other_pub_key: &PublicKey,
        encrypted: &T::Encrypted,
    ) -> Result<T, Box<dyn std::error::Error>>;

    fn auth_send(
        &mut self,
        peer_id: PeerId,
        _other_pub_key: &PublicKey,
        auth: Option<ConnectionAuthEncrypted>,
    ) {
        if let Some(peer) = self.peers().get(&peer_id) {
            let _ = peer
                .cmd_sender
                .tracked_send(PeerCmd::ConnectionAuthorizationSend(auth));
        }
    }

    fn auth_encrypt_and_send(
        &mut self,
        peer_id: PeerId,
        other_pub_key: &PublicKey,
        auth: ConnectionAuth,
    );

    fn auth_decrypt(
        &mut self,
        other_pub_key: &PublicKey,
        auth: ConnectionAuthEncrypted,
    ) -> Option<ConnectionAuth>;
}

impl P2pServiceCtx {
    pub fn pending_cmds(&self) -> usize {
        self.peers
            .iter()
            .fold(self.cmd_sender.len(), |acc, (_, peer)| {
                acc + peer.cmd_sender.len()
            })
    }
}
