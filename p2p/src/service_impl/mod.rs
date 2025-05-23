#[cfg(feature = "p2p-libp2p")]
pub mod mio;
#[cfg(feature = "p2p-webrtc")]
pub mod webrtc;
pub mod webrtc_with_libp2p;

use std::future::Future;

pub trait TaskSpawner: Send + Clone {
    fn spawn_main<F>(&self, name: &str, fut: F)
    where
        F: 'static + Send + Future<Output = ()>;
}

#[cfg(not(feature = "p2p-webrtc"))]
pub mod webrtc {
    use std::collections::BTreeMap;

    use openmina_core::channels::mpsc;

    use crate::{
        channels::{ChannelId, ChannelMsg, MsgId},
        connection::outgoing::P2pConnectionOutgoingInitOpts,
        identity::{EncryptableType, PublicKey, SecretKey},
        webrtc, P2pEvent, PeerId,
    };

    use super::TaskSpawner;

    pub struct P2pServiceCtx {
        pub cmd_sender: mpsc::UnboundedSender<Cmd>,
        pub peers: BTreeMap<PeerId, PeerState>,
    }

    pub type PeerState = ();

    pub enum Cmd {}

    #[allow(unused_variables)]
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
            _secret_key: SecretKey,
            _spawner: S,
            _rng_seed: [u8; 32],
        ) -> P2pServiceCtx {
            let (cmd_sender, _) = mpsc::unbounded_channel();
            P2pServiceCtx {
                cmd_sender,
                peers: Default::default(),
            }
        }

        fn outgoing_init(&mut self, peer_id: PeerId) {}

        fn incoming_init(&mut self, peer_id: PeerId, offer: webrtc::Offer) {}

        fn set_answer(&mut self, peer_id: PeerId, answer: webrtc::Answer) {}

        fn http_signaling_request(&mut self, url: String, offer: webrtc::Offer) {}

        fn disconnect(&mut self, peer_id: PeerId) -> bool {
            false
        }

        fn channel_open(&mut self, peer_id: PeerId, id: ChannelId) {}

        fn channel_send(&mut self, peer_id: PeerId, msg_id: MsgId, msg: ChannelMsg) {}

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
            other_pub_key: &PublicKey,
            auth: Option<webrtc::ConnectionAuthEncrypted>,
        ) {
        }

        fn auth_encrypt_and_send(
            &mut self,
            peer_id: PeerId,
            other_pub_key: &PublicKey,
            auth: webrtc::ConnectionAuth,
        );

        fn auth_decrypt(
            &mut self,
            other_pub_key: &PublicKey,
            auth: webrtc::ConnectionAuthEncrypted,
        ) -> Option<webrtc::ConnectionAuth>;
    }
}
