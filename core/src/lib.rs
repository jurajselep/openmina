extern crate graphannis_malloc_size_of as malloc_size_of;
extern crate graphannis_malloc_size_of_derive as malloc_size_of_derive;

pub mod distributed_pool;
pub mod invariants;
pub mod log;
pub mod requests;

// TODO(binier): refactor
#[cfg(target_family = "wasm")]
pub mod http;

pub mod channels;
pub mod thread;

pub mod constants;
pub mod dummy;

pub mod block;
pub mod p2p;
pub mod snark;
pub mod transaction;

pub mod consensus;

mod substate;

pub use substate::{Substate, SubstateAccess, SubstateResult};

pub mod network;
pub use network::NetworkConfig;

mod chain_id;
pub use chain_id::*;

pub mod encrypted_key;
pub use encrypted_key::*;

mod work_dir {
    use once_cell::sync::OnceCell;
    use std::path::PathBuf;

    static HOME_DIR: OnceCell<PathBuf> = OnceCell::new();

    pub fn set_work_dir(dir: PathBuf) {
        HOME_DIR.set(dir).expect("Work dir can only be set once");
    }

    pub fn get_work_dir() -> PathBuf {
        HOME_DIR.get().expect("Work dir is not set").clone()
    }

    pub fn get_debug_dir() -> PathBuf {
        get_work_dir().join("debug")
    }
}

pub use work_dir::{get_debug_dir, get_work_dir, set_work_dir};

use rand::prelude::*;
#[inline(always)]
pub fn pseudo_rng(time: redux::Timestamp) -> StdRng {
    StdRng::seed_from_u64(time.into())
}

pub fn preshared_key(chain_id: &ChainId) -> [u8; 32] {
    use multihash::Hasher;
    let mut hasher = Blake2b256::default();
    hasher.update(b"/coda/0.0.1/");
    hasher.update(chain_id.to_hex().as_bytes());
    let hash = hasher.finalize();
    let mut psk_fixed: [u8; 32] = Default::default();
    psk_fixed.copy_from_slice(hash.as_ref());
    psk_fixed
}

pub use log::ActionEvent;
use multihash::Blake2b256;
pub use openmina_macros::*;

#[cfg(feature = "fuzzing")]
pub use openmina_fuzzer::*;

#[macro_export]
macro_rules! fuzz_maybe {
    ($expr:expr, $mutator:expr) => {
        if cfg!(feature = "fuzzing") {
            $crate::fuzz!($expr, $mutator);
        }
    };
}

#[macro_export]
macro_rules! fuzzed_maybe {
    ($expr:expr, $mutator:expr) => {
        if cfg!(feature = "fuzzing") {
            $crate::fuzzed!($expr, $mutator)
        } else {
            $expr
        }
    };
}
