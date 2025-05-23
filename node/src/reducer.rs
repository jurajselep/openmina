use openmina_core::{bug_condition, error, Substate};
use p2p::{P2pAction, P2pEffectfulAction, P2pInitializeAction, P2pState};

use crate::{
    external_snark_worker::ExternalSnarkWorkers,
    rpc::RpcState,
    state::{BlockProducerState, LedgerState},
    transition_frontier::candidate::TransitionFrontierCandidateAction,
    Action, ActionWithMeta, EventSourceAction, P2p, State,
};

pub fn reducer(
    state: &mut State,
    action: &ActionWithMeta,
    dispatcher: &mut redux::Dispatcher<Action, State>,
) {
    let meta = action.meta().clone();
    match action.action() {
        Action::CheckTimeouts(_) => {
            if state.p2p.ready().is_some() {
                if let Err(error) =
                    P2pState::p2p_timeout_dispatch(Substate::new(state, dispatcher), &meta)
                {
                    bug_condition!("{}", error);
                };
            }
            dispatcher.push(TransitionFrontierCandidateAction::TransitionFrontierSyncTargetUpdate);
        }
        Action::EventSource(EventSourceAction::NewEvent { .. }) => {}
        Action::EventSource(_) => {}
        Action::P2p(a) => match a {
            P2pAction::Initialization(P2pInitializeAction::Initialize { chain_id }) => {
                if let Err(err) = state.p2p.initialize(chain_id) {
                    error!(meta.time(); summary = "error initializing p2p", error = display(err));
                }
                dispatcher.push(P2pEffectfulAction::Initialize);
            }
            p2p_action => match &mut state.p2p {
                P2p::Pending(_) => {
                    error!(meta.time(); summary = "p2p is not initialized", action = debug(p2p_action))
                }
                P2p::Ready(_) => {
                    let time = meta.time();
                    let result = p2p::P2pState::reducer(
                        Substate::new(state, dispatcher),
                        meta.with_action(p2p_action.clone()),
                    );

                    if let Err(error) = result {
                        use crate::ActionKindGet as _;
                        error!(time;
                            summary = "Failure when handling a P2P action",
                            action_kind = format!("{}", p2p_action.kind()),
                            error = display(error));
                    }
                }
            },
        },
        Action::P2pEffectful(_) => {}
        Action::Ledger(action) => {
            LedgerState::reducer(Substate::new(state, dispatcher), meta.with_action(action));
        }
        Action::LedgerEffects(_) => {}
        Action::Snark(a) => {
            snark::SnarkState::reducer(Substate::new(state, dispatcher), meta.with_action(a));
        }
        Action::TransitionFrontier(a) => {
            crate::transition_frontier::TransitionFrontierState::reducer(
                Substate::new(state, dispatcher),
                meta.with_action(a),
            );
        }
        Action::SnarkPool(a) => {
            crate::snark_pool::SnarkPoolState::reducer(
                Substate::new(state, dispatcher),
                meta.with_action(a),
            );
        }
        Action::SnarkPoolEffect(_) => {}
        Action::TransactionPool(a) => {
            crate::transaction_pool::TransactionPoolState::reducer(
                Substate::new(state, dispatcher),
                meta.with_action(a),
            );
        }
        Action::TransactionPoolEffect(_) => {}
        Action::BlockProducer(action) => {
            BlockProducerState::reducer(Substate::new(state, dispatcher), meta.with_action(action));
        }
        Action::BlockProducerEffectful(_) => {}
        Action::ExternalSnarkWorker(action) => {
            ExternalSnarkWorkers::reducer(
                Substate::new(state, dispatcher),
                meta.with_action(action),
            );
        }
        Action::ExternalSnarkWorkerEffects(_) => {}
        Action::Rpc(action) => {
            RpcState::reducer(Substate::new(state, dispatcher), meta.with_action(action));
        }
        Action::RpcEffectful(_) => {}
        Action::WatchedAccounts(a) => {
            crate::watched_accounts::WatchedAccountsState::reducer(
                Substate::new(state, dispatcher),
                meta.with_action(a),
            );
        }
        Action::P2pCallbacks(action) => {
            State::p2p_callback_reducer(Substate::new(state, dispatcher), meta.with_action(action))
        }
    }

    // must be the last.
    state.action_applied(action);
}
