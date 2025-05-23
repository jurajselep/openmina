mod rpc_service;

use std::collections::VecDeque;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use std::{collections::BTreeMap, sync::Arc};

use ledger::dummy::dummy_transaction_proof;
use ledger::proofs::transaction::ProofError;
use ledger::scan_state::scan_state::transaction_snark::SokMessage;
use ledger::scan_state::transaction_logic::{verifiable, WithStatus};
use ledger::Mask;
use mina_p2p_messages::string::ByteString;
use mina_p2p_messages::v2::{
    CurrencyFeeStableV1, LedgerHash, LedgerProofProdStableV2, MinaBaseProofStableV2,
    MinaStateSnarkedLedgerStateWithSokStableV2, NonZeroCurvePoint,
    ProverExtendBlockchainInputStableV2, SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponseA0Single,
    StateHash, TransactionSnarkStableV2, TransactionSnarkWorkTStableV2Proofs,
};
use node::account::AccountPublicKey;
use node::block_producer::vrf_evaluator::VrfEvaluatorInput;
use node::block_producer::BlockProducerEvent;
use node::core::channels::mpsc;
use node::core::invariants::InvariantsState;
use node::core::snark::{Snark, SnarkJobId};
use node::external_snark_worker_effectful::ExternalSnarkWorkerEvent;
use node::ledger::write::BlockApplyResult;
use node::p2p::service_impl::webrtc_with_libp2p::P2pServiceWebrtcWithLibp2p;
use node::p2p::P2pCryptoService;
use node::recorder::Recorder;
use node::service::{
    BlockProducerService, BlockProducerVrfEvaluatorService, TransitionFrontierGenesisService,
};
use node::snark::block_verify::{
    SnarkBlockVerifyId, SnarkBlockVerifyService, VerifiableBlockWithHash,
};
use node::snark::user_command_verify::SnarkUserCommandVerifyId;
use node::snark::user_command_verify_effectful::SnarkUserCommandVerifyService;
use node::snark::work_verify::{SnarkWorkVerifyId, SnarkWorkVerifyService};
use node::snark::{BlockVerifier, SnarkEvent, TransactionVerifier, VerifierSRS};
use node::snark_pool::SnarkPoolService;
use node::stats::Stats;
use node::transition_frontier::archive::archive_service::ArchiveService;
use node::transition_frontier::genesis::GenesisConfig;
use node::{
    event_source::Event,
    external_snark_worker::SnarkWorkSpec,
    external_snark_worker_effectful::ExternalSnarkWorkerService,
    p2p::{
        connection::outgoing::P2pConnectionOutgoingInitOpts,
        service_impl::webrtc::{Cmd, P2pServiceWebrtc, PeerState},
        webrtc, PeerId,
    },
};
use node::{ActionWithMeta, State};
use openmina_core::channels::Aborter;
use openmina_node_native::NodeService;
use redux::Instant;

use crate::cluster::{ClusterNodeId, ProofKind};
use crate::node::NonDeterministicEvent;

pub type DynEffects = Box<dyn FnMut(&State, &NodeTestingService, &ActionWithMeta) + Send>;

#[derive(Hash, Ord, PartialOrd, Eq, PartialEq)]
pub struct PendingEventIdType;
impl openmina_core::requests::RequestIdType for PendingEventIdType {
    fn request_id_type() -> &'static str {
        "PendingEventId"
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct PendingEventId(usize);

struct PendingEvents {
    events: VecDeque<(PendingEventId, Event)>,
    next_id: PendingEventId,
}

impl PendingEventId {
    fn copy_inc(&mut self) -> Self {
        let copy = *self;
        let _ = self.0.wrapping_add(1);
        copy
    }
}

impl PendingEvents {
    fn new() -> Self {
        PendingEvents {
            events: VecDeque::new(),
            next_id: Default::default(),
        }
    }

    fn add(&mut self, event: Event) -> PendingEventId {
        let id = self.next_id.copy_inc();
        self.events.push_back((id, event));
        id
    }

    fn get(&self, id: PendingEventId) -> Option<&Event> {
        self.events
            .iter()
            .find_map(|(_id, event)| (*_id == id).then_some(event))
    }

    fn remove(&mut self, id: PendingEventId) -> Option<Event> {
        if let Some(i) = self
            .events
            .iter()
            .enumerate()
            .find_map(|(i, (_id, _))| (*_id == id).then_some(i))
        {
            self.events.remove(i).map(|(_, event)| event)
        } else {
            None
        }
    }

    fn iter(&self) -> impl Iterator<Item = (PendingEventId, &Event)> {
        self.events.iter().map(|(id, event)| (*id, event))
    }
}

pub struct NodeTestingService {
    real: NodeService,
    id: ClusterNodeId,
    /// Use webrtc p2p between Rust nodes.
    rust_to_rust_use_webrtc: bool,
    proof_kind: ProofKind,
    /// We are replaying this node so disable some non-deterministic services.
    is_replay: bool,
    monotonic_time: Instant,
    /// Events sent by the real service not yet received by state machine.
    pending_events: PendingEvents,
    //pending_events: PendingRequests<PendingEventIdType, Event>,
    dyn_effects: Option<DynEffects>,

    snarker_sok_digest: Option<ByteString>,

    cluster_invariants_state: Arc<StdMutex<InvariantsState>>,
    /// Once dropped, it will cause all threads associated to shutdown.
    _shutdown: Aborter,
}

impl NodeTestingService {
    pub fn new(
        real: NodeService,
        id: ClusterNodeId,
        cluster_invariants_state: Arc<StdMutex<InvariantsState>>,
        _shutdown: Aborter,
    ) -> Self {
        Self {
            real,
            id,
            rust_to_rust_use_webrtc: false,
            proof_kind: ProofKind::default(),
            is_replay: false,
            monotonic_time: Instant::now(),
            pending_events: PendingEvents::new(),
            dyn_effects: None,
            snarker_sok_digest: None,
            cluster_invariants_state,
            _shutdown,
        }
    }

    pub fn node_id(&self) -> ClusterNodeId {
        self.id
    }

    pub fn rust_to_rust_use_webrtc(&self) -> bool {
        self.rust_to_rust_use_webrtc
    }

    pub fn set_rust_to_rust_use_webrtc(&mut self) -> &mut Self {
        assert!(cfg!(feature = "p2p-webrtc"));
        self.rust_to_rust_use_webrtc = true;
        self
    }

    pub fn proof_kind(&self) -> ProofKind {
        self.proof_kind
    }

    pub fn set_proof_kind(&mut self, kind: ProofKind) -> &mut Self {
        self.proof_kind = kind;
        self
    }

    pub fn set_replay(&mut self) -> &mut Self {
        self.is_replay = true;
        self
    }

    pub fn advance_time(&mut self, by_nanos: u64) {
        self.monotonic_time += Duration::from_nanos(by_nanos);
    }

    pub fn dyn_effects(&mut self, state: &State, action: &ActionWithMeta) {
        if let Some(mut dyn_effects) = self.dyn_effects.take() {
            (dyn_effects)(state, self, action);
            self.dyn_effects = Some(dyn_effects);
        }
    }

    pub fn set_dyn_effects(&mut self, effects: DynEffects) {
        self.dyn_effects = Some(effects);
    }

    pub fn remove_dyn_effects(&mut self) -> Option<DynEffects> {
        self.dyn_effects.take()
    }

    pub fn set_snarker_sok_digest(&mut self, digest: ByteString) {
        self.snarker_sok_digest = Some(digest);
    }

    pub fn pending_events(&mut self, poll: bool) -> impl Iterator<Item = (PendingEventId, &Event)> {
        while let Ok(req) = self.real.rpc_receiver().try_recv() {
            self.real.process_rpc_request(req);
        }
        if poll {
            while let Some(event) = self.real.event_receiver().try_next() {
                // Drop non-deterministic events during replay. We
                // have those recorded as `ScenarioStep::NonDeterministicEvent`.
                if self.is_replay && NonDeterministicEvent::should_drop_event(&event) {
                    eprintln!("dropping non-deterministic event: {event:?}");
                    continue;
                }
                self.pending_events.add(event);
            }
        }
        self.pending_events.iter()
    }

    pub async fn next_pending_event(&mut self) -> Option<(PendingEventId, &Event)> {
        let event = loop {
            let (event_receiver, rpc_receiver) = self.real.event_receiver_with_rpc_receiver();
            tokio::select! {
                Some(rpc) = rpc_receiver.recv() => {
                    self.real.process_rpc_request(rpc);
                    break self.real.event_receiver().try_next().unwrap();
                }
                res = event_receiver.wait_for_events() => {
                    res.ok()?;
                    let event = self.real.event_receiver().try_next().unwrap();
                    // Drop non-deterministic events during replay. We
                    // have those recorded as `ScenarioStep::NonDeterministicEvent`.
                    if self.is_replay && NonDeterministicEvent::should_drop_event(&event) {
                        eprintln!("dropping non-deterministic event: {event:?}");
                        continue;
                    }
                    break event;
                }
            }
        };
        let id = self.pending_events.add(event);
        Some((id, self.pending_events.get(id).unwrap()))
    }

    pub fn get_pending_event(&self, id: PendingEventId) -> Option<&Event> {
        self.pending_events.get(id)
    }

    pub fn take_pending_event(&mut self, id: PendingEventId) -> Option<Event> {
        self.pending_events.remove(id)
    }

    pub fn ledger(&self, ledger_hash: &LedgerHash) -> Option<Mask> {
        self.real
            .ledger_manager()
            .get_mask(ledger_hash)
            .map(|(mask, _)| mask)
    }
}

impl redux::Service for NodeTestingService {}

impl node::Service for NodeTestingService {
    fn queues(&mut self) -> node::service::Queues {
        self.real.queues()
    }

    fn stats(&mut self) -> Option<&mut Stats> {
        self.real.stats()
    }

    fn recorder(&mut self) -> &mut Recorder {
        self.real.recorder()
    }

    fn is_replay(&self) -> bool {
        self.is_replay
    }
}

impl P2pCryptoService for NodeTestingService {
    fn generate_random_nonce(&mut self) -> [u8; 24] {
        self.real.generate_random_nonce()
    }

    fn ephemeral_sk(&mut self) -> [u8; 32] {
        self.real.ephemeral_sk()
    }

    fn static_sk(&mut self) -> [u8; 32] {
        self.real.static_sk()
    }

    fn sign_key(&mut self, key: &[u8; 32]) -> Vec<u8> {
        self.real.sign_key(key)
    }

    fn sign_publication(&mut self, publication: &[u8]) -> Vec<u8> {
        self.real.sign_publication(publication)
    }

    fn verify_publication(
        &mut self,
        pk: &libp2p_identity::PublicKey,
        publication: &[u8],
        sig: &[u8],
    ) -> bool {
        self.real.verify_publication(pk, publication, sig)
    }
}

impl node::ledger::LedgerService for NodeTestingService {
    fn ledger_manager(&self) -> &node::ledger::LedgerManager {
        self.real.ledger_manager()
    }
}

impl redux::TimeService for NodeTestingService {
    fn monotonic_time(&mut self) -> Instant {
        self.monotonic_time
    }
}

impl node::event_source::EventSourceService for NodeTestingService {
    fn next_event(&mut self) -> Option<Event> {
        None
    }
}

impl TransitionFrontierGenesisService for NodeTestingService {
    fn load_genesis(&mut self, config: Arc<GenesisConfig>) {
        TransitionFrontierGenesisService::load_genesis(&mut self.real, config);
    }
}

impl P2pServiceWebrtc for NodeTestingService {
    type Event = Event;

    fn random_pick(
        &mut self,
        list: &[P2pConnectionOutgoingInitOpts],
    ) -> Option<P2pConnectionOutgoingInitOpts> {
        self.real.random_pick(list)
    }

    fn event_sender(&self) -> &mpsc::UnboundedSender<Event> {
        P2pServiceWebrtc::event_sender(&self.real)
    }

    fn cmd_sender(&self) -> &mpsc::TrackedUnboundedSender<Cmd> {
        P2pServiceWebrtc::cmd_sender(&self.real)
    }

    fn peers(&mut self) -> &mut BTreeMap<PeerId, PeerState> {
        P2pServiceWebrtc::peers(&mut self.real)
    }

    fn outgoing_init(&mut self, peer_id: PeerId) {
        P2pServiceWebrtc::outgoing_init(&mut self.real, peer_id)
    }

    fn incoming_init(&mut self, peer_id: PeerId, offer: webrtc::Offer) {
        P2pServiceWebrtc::incoming_init(&mut self.real, peer_id, offer)
    }

    fn encrypt<T: node::p2p::identity::EncryptableType>(
        &mut self,
        other_pk: &node::p2p::identity::PublicKey,
        message: &T,
    ) -> Result<T::Encrypted, Box<dyn std::error::Error>> {
        self.real.encrypt(other_pk, message)
    }

    fn decrypt<T: node::p2p::identity::EncryptableType>(
        &mut self,
        other_pub_key: &node::p2p::identity::PublicKey,
        encrypted: &T::Encrypted,
    ) -> Result<T, Box<dyn std::error::Error>> {
        self.real.decrypt(other_pub_key, encrypted)
    }

    fn auth_encrypt_and_send(
        &mut self,
        peer_id: PeerId,
        other_pub_key: &node::p2p::identity::PublicKey,
        auth: webrtc::ConnectionAuth,
    ) {
        self.real
            .auth_encrypt_and_send(peer_id, other_pub_key, auth)
    }

    fn auth_decrypt(
        &mut self,
        other_pub_key: &node::p2p::identity::PublicKey,
        auth: webrtc::ConnectionAuthEncrypted,
    ) -> Option<webrtc::ConnectionAuth> {
        self.real.auth_decrypt(other_pub_key, auth)
    }
}

impl P2pServiceWebrtcWithLibp2p for NodeTestingService {
    #[cfg(feature = "p2p-libp2p")]
    fn mio(&mut self) -> &mut node::p2p::service_impl::mio::MioService {
        self.real.mio()
    }

    fn connections(&self) -> std::collections::BTreeSet<PeerId> {
        self.real.connections()
    }
}

impl SnarkBlockVerifyService for NodeTestingService {
    fn verify_init(
        &mut self,
        req_id: SnarkBlockVerifyId,
        verifier_index: BlockVerifier,
        verifier_srs: Arc<VerifierSRS>,
        block: VerifiableBlockWithHash,
    ) {
        match self.proof_kind() {
            ProofKind::Dummy | ProofKind::ConstraintsChecked => {
                let _ = self
                    .real
                    .event_sender()
                    .send(SnarkEvent::BlockVerify(req_id, Ok(())).into());
            }
            ProofKind::Full => SnarkBlockVerifyService::verify_init(
                &mut self.real,
                req_id,
                verifier_index,
                verifier_srs,
                block,
            ),
        }
    }
}

impl SnarkUserCommandVerifyService for NodeTestingService {
    fn verify_init(
        &mut self,
        req_id: SnarkUserCommandVerifyId,
        commands: Vec<WithStatus<verifiable::UserCommand>>,
    ) {
        SnarkUserCommandVerifyService::verify_init(&mut self.real, req_id, commands)
    }
}

impl SnarkWorkVerifyService for NodeTestingService {
    fn verify_init(
        &mut self,
        req_id: SnarkWorkVerifyId,
        verifier_index: TransactionVerifier,
        verifier_srs: Arc<VerifierSRS>,
        work: Vec<Snark>,
    ) {
        match self.proof_kind() {
            ProofKind::Dummy | ProofKind::ConstraintsChecked => {
                let _ = self
                    .real
                    .event_sender()
                    .send(SnarkEvent::WorkVerify(req_id, Ok(())).into());
            }
            ProofKind::Full => SnarkWorkVerifyService::verify_init(
                &mut self.real,
                req_id,
                verifier_index,
                verifier_srs,
                work,
            ),
        }
    }
}

impl SnarkPoolService for NodeTestingService {
    fn random_choose<'a>(
        &mut self,
        iter: impl Iterator<Item = &'a SnarkJobId>,
        n: usize,
    ) -> Vec<SnarkJobId> {
        self.real.random_choose(iter, n)
    }
}

impl BlockProducerVrfEvaluatorService for NodeTestingService {
    fn evaluate(&mut self, data: VrfEvaluatorInput) {
        BlockProducerVrfEvaluatorService::evaluate(&mut self.real, data)
    }
}

impl ArchiveService for NodeTestingService {
    fn send_to_archive(&mut self, data: BlockApplyResult) {
        self.real.send_to_archive(data);
    }
}

use std::cell::RefCell;
thread_local! {
    static GENESIS_PROOF: RefCell<Option<(StateHash, Arc<MinaBaseProofStableV2>)>> = const { RefCell::new(None)};
}

impl BlockProducerService for NodeTestingService {
    fn provers(&self) -> ledger::proofs::provers::BlockProver {
        self.real.provers()
    }

    fn prove(
        &mut self,
        block_hash: StateHash,
        mut input: Box<ProverExtendBlockchainInputStableV2>,
    ) {
        fn dummy_proof_event(block_hash: StateHash) -> Event {
            let dummy_proof = (*ledger::dummy::dummy_blockchain_proof()).clone();
            BlockProducerEvent::BlockProve(block_hash, Ok(dummy_proof.into())).into()
        }
        let keypair = self.real.block_producer().unwrap().keypair();

        match self.proof_kind() {
            ProofKind::Dummy => {
                let _ = self.real.event_sender().send(dummy_proof_event(block_hash));
            }
            ProofKind::ConstraintsChecked => {
                match openmina_node_native::block_producer::prove(
                    self.provers(),
                    &mut input,
                    &keypair,
                    true,
                ) {
                    Err(e)
                        if matches!(
                            e.downcast_ref::<ProofError>(),
                            Some(ProofError::ConstraintsOk)
                        ) =>
                    {
                        let _ = self.real.event_sender().send(dummy_proof_event(block_hash));
                    }
                    Err(err) => panic!("unexpected block proof generation error: {err:?}"),
                    Ok(_) => unreachable!(),
                }
            }
            ProofKind::Full => {
                // TODO(binier): handle if block is genesis based on fork constants.
                let is_genesis = input
                    .next_state
                    .body
                    .consensus_state
                    .blockchain_length
                    .as_u32()
                    == 1;
                let res = GENESIS_PROOF.with_borrow_mut(|cached_genesis| {
                    if let Some((_, proof)) = cached_genesis
                        .as_ref()
                        .filter(|(hash, _)| is_genesis && hash == &block_hash)
                    {
                        Ok(proof.clone())
                    } else {
                        openmina_node_native::block_producer::prove(
                            self.provers(),
                            &mut input,
                            &keypair,
                            false,
                        )
                        .map_err(|err| format!("{err:?}"))
                    }
                });
                if let Some(proof) = res.as_ref().ok().filter(|_| is_genesis) {
                    GENESIS_PROOF
                        .with_borrow_mut(|data| *data = Some((block_hash.clone(), proof.clone())));
                }
                let _ = self
                    .real
                    .event_sender()
                    .send(BlockProducerEvent::BlockProve(block_hash, res).into());
            }
        }
    }

    fn with_producer_keypair<T>(
        &self,
        _f: impl FnOnce(&node::account::AccountSecretKey) -> T,
    ) -> Option<T> {
        None
    }
}

impl ExternalSnarkWorkerService for NodeTestingService {
    fn start(
        &mut self,
        public_key: NonZeroCurvePoint,
        fee: CurrencyFeeStableV1,
        _: TransactionVerifier,
    ) -> Result<(), node::external_snark_worker::ExternalSnarkWorkerError> {
        let pub_key = AccountPublicKey::from(public_key);
        let sok_message = SokMessage::create(
            (&fee).into(),
            pub_key.try_into().map_err(|e| {
                node::external_snark_worker::ExternalSnarkWorkerError::Error(format!("{:?}", e))
            })?,
        );
        self.set_snarker_sok_digest((&sok_message.digest()).into());
        let _ = self
            .real
            .event_sender()
            .send(ExternalSnarkWorkerEvent::Started.into());
        Ok(())
        // self.real.start(path, public_key, fee)
    }

    fn submit(
        &mut self,
        spec: SnarkWorkSpec,
    ) -> Result<(), node::external_snark_worker::ExternalSnarkWorkerError> {
        let sok_digest = self.snarker_sok_digest.clone().unwrap();
        let make_dummy_proof = |spec| {
            let statement = match spec {
                SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponseA0Single::Transition(v, _) => v.0,
                SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponseA0Single::Merge(v) => v.0 .0,
            };

            LedgerProofProdStableV2(TransactionSnarkStableV2 {
                statement: MinaStateSnarkedLedgerStateWithSokStableV2 {
                    source: statement.source,
                    target: statement.target,
                    connecting_ledger_left: statement.connecting_ledger_left,
                    connecting_ledger_right: statement.connecting_ledger_right,
                    supply_increase: statement.supply_increase,
                    fee_excess: statement.fee_excess,
                    sok_digest: sok_digest.clone(),
                },
                proof: (*dummy_transaction_proof()).clone(),
            })
        };
        let res = match spec {
            SnarkWorkSpec::One(v) => TransactionSnarkWorkTStableV2Proofs::One(make_dummy_proof(v)),
            SnarkWorkSpec::Two((v1, v2)) => TransactionSnarkWorkTStableV2Proofs::Two((
                make_dummy_proof(v1),
                make_dummy_proof(v2),
            )),
        };
        let _ = self
            .real
            .event_sender()
            .send(ExternalSnarkWorkerEvent::WorkResult(Arc::new(res)).into());
        Ok(())
        // self.real.submit(spec)
    }

    fn cancel(&mut self) -> Result<(), node::external_snark_worker::ExternalSnarkWorkerError> {
        let _ = self
            .real
            .event_sender()
            .send(ExternalSnarkWorkerEvent::WorkCancelled.into());
        Ok(())
        // self.real.cancel()
    }

    fn kill(&mut self) -> Result<(), node::external_snark_worker::ExternalSnarkWorkerError> {
        let _ = self
            .real
            .event_sender()
            .send(ExternalSnarkWorkerEvent::Killed.into());
        Ok(())
        // self.real.kill()
    }
}

impl node::core::invariants::InvariantService for NodeTestingService {
    type ClusterInvariantsState<'a> = std::sync::MutexGuard<'a, InvariantsState>;

    fn node_id(&self) -> usize {
        self.node_id().index()
    }

    fn invariants_state(&mut self) -> &mut InvariantsState {
        node::core::invariants::InvariantService::invariants_state(&mut self.real)
    }

    fn cluster_invariants_state<'a>(&'a mut self) -> Option<Self::ClusterInvariantsState<'a>>
    where
        Self: 'a,
    {
        Some(
            self.cluster_invariants_state.try_lock().expect(
                "locking should never fail, since we are running all nodes in the same thread",
            ),
        )
    }
}
