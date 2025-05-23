use std::cmp::Ordering;

use openmina_core::snark::{Snark, SnarkInfo, SnarkJobId};
use openmina_core::ActionEvent;
use serde::{Deserialize, Serialize};

use crate::p2p::channels::rpc::P2pRpcId;
use crate::p2p::PeerId;
use crate::snark::work_verify::SnarkWorkVerifyId;

use super::SnarkPoolCandidateState;

pub type SnarkPoolCandidateActionWithMeta = redux::ActionWithMeta<SnarkPoolCandidateAction>;
pub type SnarkPoolCandidateActionWithMetaRef<'a> =
    redux::ActionWithMeta<&'a SnarkPoolCandidateAction>;

#[derive(Serialize, Deserialize, Debug, Clone, ActionEvent)]
pub enum SnarkPoolCandidateAction {
    InfoReceived {
        peer_id: PeerId,
        info: SnarkInfo,
    },
    #[action_event(level = trace)]
    WorkFetchAll,
    WorkFetchInit {
        peer_id: PeerId,
        job_id: SnarkJobId,
    },
    WorkFetchPending {
        peer_id: PeerId,
        job_id: SnarkJobId,
        rpc_id: P2pRpcId,
    },
    WorkFetchError {
        peer_id: PeerId,
        job_id: SnarkJobId,
    },
    WorkFetchSuccess {
        peer_id: PeerId,
        work: Snark,
    },
    #[action_event(level = trace)]
    WorkVerifyNext,
    WorkVerifyPending {
        peer_id: PeerId,
        job_ids: Vec<SnarkJobId>,
        verify_id: SnarkWorkVerifyId,
    },
    WorkVerifyError {
        peer_id: PeerId,
        verify_id: SnarkWorkVerifyId,
        batch: Vec<SnarkJobId>,
    },
    WorkVerifySuccess {
        peer_id: PeerId,
        verify_id: SnarkWorkVerifyId,
        batch: Vec<Snark>,
    },
    PeerPrune {
        peer_id: PeerId,
    },
}

impl redux::EnablingCondition<crate::State> for SnarkPoolCandidateAction {
    fn is_enabled(&self, state: &crate::State, _time: redux::Timestamp) -> bool {
        match self {
            SnarkPoolCandidateAction::InfoReceived { peer_id, info } => {
                state.snark_pool.contains(&info.job_id)
                    && state
                        .snark_pool
                        .candidates
                        .get(*peer_id, &info.job_id)
                        .is_none_or(|v| info > v)
            }
            SnarkPoolCandidateAction::WorkFetchAll => state.p2p.ready().is_some(),
            SnarkPoolCandidateAction::WorkFetchInit { peer_id, job_id } => {
                let is_peer_available = state
                    .p2p
                    .get_ready_peer(peer_id)
                    .is_some_and(|peer| peer.channels.rpc.can_send_request());
                is_peer_available
                    && state
                        .snark_pool
                        .candidates
                        .get(*peer_id, job_id)
                        .is_some_and(|s| matches!(s, SnarkPoolCandidateState::InfoReceived { .. }))
            }
            SnarkPoolCandidateAction::WorkFetchPending {
                peer_id, job_id, ..
            } => state
                .snark_pool
                .candidates
                .get(*peer_id, job_id)
                .is_some_and(|s| matches!(s, SnarkPoolCandidateState::InfoReceived { .. })),
            SnarkPoolCandidateAction::WorkFetchError { peer_id, job_id } => state
                .snark_pool
                .candidates
                .get(*peer_id, job_id)
                .is_some_and(|s| matches!(s, SnarkPoolCandidateState::WorkFetchPending { .. })),
            SnarkPoolCandidateAction::WorkFetchSuccess { peer_id, work } => {
                let job_id = work.job_id();
                state.snark_pool.contains(&job_id)
                    && state
                        .snark_pool
                        .candidates
                        .get(*peer_id, &job_id)
                        .is_none_or(|v| match work.partial_cmp(v).unwrap() {
                            Ordering::Less => false,
                            Ordering::Greater => true,
                            Ordering::Equal => {
                                matches!(v, SnarkPoolCandidateState::WorkFetchPending { .. })
                            }
                        })
            }
            SnarkPoolCandidateAction::WorkVerifyNext => {
                state.snark.work_verify.jobs.is_empty()
                    && state.transition_frontier.sync.is_synced()
            }
            SnarkPoolCandidateAction::WorkVerifyPending {
                peer_id, job_ids, ..
            } => {
                !job_ids.is_empty()
                    && state
                        .snark_pool
                        .candidates
                        .jobs_from_peer_with_job_ids(*peer_id, job_ids)
                        .all(|(_, state)| {
                            matches!(state, Some(SnarkPoolCandidateState::WorkReceived { .. }))
                        })
            }
            SnarkPoolCandidateAction::WorkVerifyError { .. } => {
                // TODO(binier)
                true
            }
            SnarkPoolCandidateAction::WorkVerifySuccess { .. } => {
                // TODO(binier)
                true
            }
            SnarkPoolCandidateAction::PeerPrune { peer_id } => {
                state.snark_pool.candidates.peer_work_count(peer_id) > 0
            }
        }
    }
}

use crate::snark_pool::SnarkPoolAction;

impl From<SnarkPoolCandidateAction> for crate::Action {
    fn from(value: SnarkPoolCandidateAction) -> Self {
        Self::SnarkPool(SnarkPoolAction::Candidate(value))
    }
}
