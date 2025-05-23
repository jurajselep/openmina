use std::{fmt, io, sync::Arc};

use ark_ff::fields::arithmetic::InvalidBigInt;
use binprot::{BinProtRead, BinProtWrite};
use generated::MinaStateBlockchainStateValueStableV2;
use mina_hasher::Fp;
use poseidon::hash::{
    hash_with_kimchi,
    params::{MINA_PROTO_STATE, MINA_PROTO_STATE_BODY},
    Inputs,
};
use serde::{Deserialize, Serialize};
use sha2::{
    digest::{generic_array::GenericArray, typenum::U32},
    Digest, Sha256,
};

use crate::{bigint::BigInt, hash::MinaHash, hash_input::FailableToInputs};

use super::{
    generated, ConsensusBodyReferenceStableV1, ConsensusGlobalSlotStableV1,
    ConsensusProofOfStakeDataConsensusStateValueStableV2,
    ConsensusProofOfStakeDataEpochDataNextValueVersionedValueStableV1,
    ConsensusProofOfStakeDataEpochDataStakingValueVersionedValueStableV1,
    ConsensusVrfOutputTruncatedStableV1, DataHashLibStateHashStableV1, LedgerHash,
    MinaBaseControlStableV2, MinaBaseEpochLedgerValueStableV1, MinaBaseFeeExcessStableV1,
    MinaBaseLedgerHash0StableV1, MinaBasePendingCoinbaseHashBuilderStableV1,
    MinaBasePendingCoinbaseHashVersionedStableV1, MinaBasePendingCoinbaseStackVersionedStableV1,
    MinaBasePendingCoinbaseStateStackStableV1, MinaBaseProtocolConstantsCheckedValueStableV1,
    MinaBaseStagedLedgerHashNonSnarkStableV1, MinaBaseStagedLedgerHashStableV1,
    MinaBaseStateBodyHashStableV1, MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAA,
    MinaNumbersGlobalSlotSinceGenesisMStableV1, MinaNumbersGlobalSlotSinceHardForkMStableV1,
    MinaNumbersGlobalSlotSpanStableV1, MinaStateBlockchainStateValueStableV2LedgerProofStatement,
    MinaStateBlockchainStateValueStableV2LedgerProofStatementSource,
    MinaStateBlockchainStateValueStableV2SignedAmount, MinaStateProtocolStateBodyValueStableV2,
    MinaStateProtocolStateValueStableV2,
    MinaTransactionLogicZkappCommandLogicLocalStateValueStableV1,
    NonZeroCurvePointUncompressedStableV1, PendingCoinbaseHash, SgnStableV1, SignedAmount,
    StateHash, TokenFeeExcess,
};

impl generated::MinaBaseStagedLedgerHashNonSnarkStableV1 {
    pub fn sha256(&self) -> GenericArray<u8, U32> {
        let mut ledger_hash_bytes: [u8; 32] = [0; 32];

        ledger_hash_bytes.copy_from_slice(&self.ledger_hash.to_bytes()[..]);
        ledger_hash_bytes.reverse();

        let mut hasher = Sha256::new();
        hasher.update(ledger_hash_bytes);
        hasher.update(self.aux_hash.as_ref());
        hasher.update(self.pending_coinbase_aux.as_ref());

        hasher.finalize()
    }
}

impl generated::ConsensusVrfOutputTruncatedStableV1 {
    pub fn blake2b(&self) -> Vec<u8> {
        use blake2::{
            digest::{Update, VariableOutput},
            Blake2bVar,
        };
        let mut hasher = Blake2bVar::new(32).expect("Invalid Blake2bVar output size");
        hasher.update(&self.0);
        hasher.finalize_boxed().to_vec()
    }
}

#[derive(Hash, Eq, PartialEq, Ord, PartialOrd, Clone)]
pub struct TransactionHash(Arc<[u8; 32]>);

impl std::str::FromStr for TransactionHash {
    type Err = bs58::decode::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = bs58::decode(s).with_check(Some(0x1D)).into_vec()?;
        dbg!(bytes.len());
        let bytes = (&bytes[2..])
            .try_into()
            .map_err(|_| bs58::decode::Error::BufferTooSmall)?;
        Ok(Self(Arc::new(bytes)))
    }
}

impl fmt::Display for TransactionHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut bytes = [32; 33];
        bytes[1..].copy_from_slice(&*self.0);
        bs58::encode(bytes)
            .with_check_version(0x1D)
            .into_string()
            .fmt(f)
    }
}

impl fmt::Debug for TransactionHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
        // write!(f, "TransactionHash({})", self)
    }
}

impl From<&[u8; 32]> for TransactionHash {
    fn from(value: &[u8; 32]) -> Self {
        Self(Arc::new(*value))
    }
}

impl Serialize for TransactionHash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if serializer.is_human_readable() {
            serializer.serialize_str(&self.to_string())
        } else {
            serde_bytes::serialize(&*self.0, serializer)
        }
    }
}

impl<'de> serde::Deserialize<'de> for TransactionHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            let b58: String = Deserialize::deserialize(deserializer)?;
            Ok(b58.parse().map_err(|err| serde::de::Error::custom(err))?)
        } else {
            serde_bytes::deserialize(deserializer)
                .map(Arc::new)
                .map(Self)
        }
    }
}

impl BinProtWrite for TransactionHash {
    fn binprot_write<W: io::Write>(&self, w: &mut W) -> std::io::Result<()> {
        w.write_all(&*self.0)
    }
}

impl BinProtRead for TransactionHash {
    fn binprot_read<R: io::Read + ?Sized>(r: &mut R) -> Result<Self, binprot::Error>
    where
        Self: Sized,
    {
        let mut bytes = [0; 32];
        r.read_exact(&mut bytes)?;
        Ok(Self(bytes.into()))
    }
}

impl generated::MinaTransactionTransactionStableV2 {
    pub fn hash(&self) -> io::Result<TransactionHash> {
        match self {
            Self::Command(v) => v.hash(),
            Self::FeeTransfer(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "fee transfer tx hashing is not yet supported",
            )),
            Self::Coinbase(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "coinbase tx hashing is not yet supported",
            )),
        }
    }
}

impl generated::MinaBaseUserCommandStableV2 {
    pub fn hash(&self) -> io::Result<TransactionHash> {
        match self {
            Self::SignedCommand(v) => v.hash(),
            Self::ZkappCommand(v) => v.hash(),
        }
    }
}

impl generated::MinaBaseSignedCommandStableV2 {
    pub fn binprot_write_with_default_sig(&self) -> io::Result<Vec<u8>> {
        let default_signature = generated::MinaBaseSignatureStableV1(BigInt::one(), BigInt::one());

        let mut encoded = vec![];
        self.payload.binprot_write(&mut encoded)?;
        self.signer.binprot_write(&mut encoded)?;
        default_signature.binprot_write(&mut encoded)?;
        Ok(encoded)
    }

    pub fn hash(&self) -> io::Result<TransactionHash> {
        use blake2::{
            digest::{Update, VariableOutput},
            Blake2bVar,
        };
        let mut hasher = Blake2bVar::new(32).expect("Invalid Blake2bVar output size");

        hasher.update(&self.binprot_write_with_default_sig()?);
        let mut hash = [0; 32];
        hasher
            .finalize_variable(&mut hash)
            .expect("Invalid buffer size"); // Never occur

        Ok(TransactionHash(hash.into()))
    }
}

// TODO(adonagy): reduce duplication
impl generated::MinaBaseZkappCommandTStableV1WireStableV1 {
    fn binprot_write_with_default(&self) -> io::Result<Vec<u8>> {
        let default_signature = generated::MinaBaseSignatureStableV1(BigInt::one(), BigInt::one());

        let mut encoded = vec![];

        let mut modified = self.clone();

        modified.fee_payer.authorization = default_signature.clone().into();

        modified.account_updates.iter_mut().for_each(|u| {
            Self::replace_auth_recursive(&mut u.elt);
        });

        modified.binprot_write(&mut encoded)?;
        Ok(encoded)
    }

    fn replace_auth_recursive(
        update_elt: &mut MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAA,
    ) {
        Self::replace_auth(&mut update_elt.account_update.authorization);
        // if update_elt.calls.is_empty() {
        //     return;
        // }

        update_elt.calls.iter_mut().for_each(|call| {
            Self::replace_auth_recursive(&mut call.elt);
        });

        // for call in update_elt.calls.iter_mut() {
        //     Self::replace_auth_recursive(&mut call.elt);
        // }
    }

    pub fn replace_auth(auth: &mut MinaBaseControlStableV2) {
        let default_signature = generated::MinaBaseSignatureStableV1(BigInt::one(), BigInt::one());
        let default_proof = super::dummy_transaction_proof();
        *auth = match auth {
            MinaBaseControlStableV2::Proof(_) => {
                MinaBaseControlStableV2::Proof(Box::new(default_proof.0.clone().into()))
            }
            MinaBaseControlStableV2::Signature(_) => {
                MinaBaseControlStableV2::Signature(default_signature.clone().into())
            }
            MinaBaseControlStableV2::NoneGiven => MinaBaseControlStableV2::NoneGiven,
        };
    }

    pub fn hash(&self) -> io::Result<TransactionHash> {
        use blake2::{
            digest::{Update, VariableOutput},
            Blake2bVar,
        };
        let mut hasher = Blake2bVar::new(32).expect("Invalid Blake2bVar output size");

        hasher.update(&self.binprot_write_with_default()?);
        let mut hash = [0; 32];
        hasher
            .finalize_variable(&mut hash)
            .expect("Invalid buffer size"); // Never occur

        Ok(TransactionHash(hash.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::super::manual;
    use super::*;
    use manual::MinaBaseSignedCommandMemoStableV1;

    fn pub_key(address: &str) -> manual::NonZeroCurvePoint {
        let key = mina_signer::PubKey::from_address(address)
            .unwrap()
            .into_compressed();
        let v = generated::NonZeroCurvePointUncompressedStableV1 {
            x: crate::bigint::BigInt::from(key.x),
            is_odd: key.is_odd,
        };
        v.into()
    }

    fn tx_hash(
        from: &str,
        to: &str,
        amount: u64,
        fee: u64,
        nonce: u32,
        valid_until: u32,
    ) -> String {
        use crate::number::Number;
        use crate::string::CharString;

        let from = pub_key(from);
        let to = pub_key(to);

        let v = Number(fee);
        let v = generated::UnsignedExtendedUInt64Int64ForVersionTagsStableV1(v);
        let fee = generated::CurrencyFeeStableV1(v);

        let nonce = generated::UnsignedExtendedUInt32StableV1(Number(nonce));

        let valid_until = generated::UnsignedExtendedUInt32StableV1(Number(valid_until));

        let memo = bs58::decode("E4Yks7aARFemZJqucP5eaARRYRthGdzaFjGfXqQRS3UeidsECRBvR")
            .with_check(Some(0x14))
            .into_vec()
            .unwrap()[1..]
            .to_vec();
        let v = CharString::from(&memo[..]);
        let memo = MinaBaseSignedCommandMemoStableV1(v);

        let common = generated::MinaBaseSignedCommandPayloadCommonStableV2 {
            fee,
            fee_payer_pk: from.clone(),
            nonce,
            valid_until: MinaNumbersGlobalSlotSinceGenesisMStableV1::SinceGenesis(valid_until),
            memo,
        };

        let v = Number(amount);
        let v = generated::UnsignedExtendedUInt64Int64ForVersionTagsStableV1(v);
        let amount = generated::CurrencyAmountStableV1(v);

        let v = generated::MinaBasePaymentPayloadStableV2 {
            receiver_pk: to.clone(),
            amount,
        };
        let body = generated::MinaBaseSignedCommandPayloadBodyStableV2::Payment(v);

        let payload = generated::MinaBaseSignedCommandPayloadStableV2 { common, body };

        // Some random signature. hasher should ignore it and use default.
        let signature = generated::MinaBaseSignatureStableV1(
            BigInt::binprot_read(&mut &[122; 32][..]).unwrap(),
            BigInt::binprot_read(&mut &[123; 32][..]).unwrap(),
        );

        let v = generated::MinaBaseSignedCommandStableV2 {
            payload,
            signer: from.clone(),
            signature: signature.into(),
        };
        let v = generated::MinaBaseUserCommandStableV2::SignedCommand(v);
        dbg!(v.hash().unwrap()).to_string()
    }

    // #[test]
    // fn test_tx_hash() {
    //     let s = "5JuSRViCY1GbnnpExLhoYLkD96vwnA97ZrbE2UFTFzBk9SPLsAyE";
    //     let hash: TransactionHash = s.parse().unwrap();
    //     // let bytes = bs58::decode(s).with_check(Some(0x1D)).into_vec().unwrap()[1..].to_vec();
    //     dbg!(bs58::encode(&hash.0)
    //         .with_check_version(0x12)
    //         .into_string());
    //     panic!();
    // }

    #[test]
    #[ignore = "fix expected hash/hasing"]
    fn test_payment_hash_1() {
        let expected_hash = "5JthQdVqzEJRLBLALeuwPdbnGhFmCow2bVnkfHGH6vZ7R6fiMf2o";
        let expected_tx_hash: TransactionHash = expected_hash.parse().unwrap();
        dbg!(expected_tx_hash);

        assert_eq!(
            tx_hash(
                "B62qp3B9VW1ir5qL1MWRwr6ecjC2NZbGr8vysGeme9vXGcFXTMNXb2t",
                "B62qoieQNrsNKCNTZ6R4D6cib3NxVbkwZaAtRVbfS3ndrb2MkFJ1UVJ",
                1089541195,
                89541195,
                26100,
                u32::MAX,
            ),
            expected_hash
        )
    }
}

fn fp_state_hash_from_fp_hashes(previous_state_hash: Fp, body_hash: Fp) -> Fp {
    let mut inputs = Inputs::new();
    inputs.append_field(previous_state_hash);
    inputs.append_field(body_hash);
    hash_with_kimchi(&MINA_PROTO_STATE, &inputs.to_fields())
}

impl StateHash {
    pub fn from_fp(fp: Fp) -> Self {
        DataHashLibStateHashStableV1(fp.into()).into()
    }

    pub fn try_from_hashes(
        pred_state_hash: &StateHash,
        body_hash: &MinaBaseStateBodyHashStableV1,
    ) -> Result<Self, InvalidBigInt> {
        Ok(Self::from_fp(fp_state_hash_from_fp_hashes(
            pred_state_hash.to_field()?,
            body_hash.to_field()?,
        )))
    }
}

impl LedgerHash {
    pub fn from_fp(fp: Fp) -> Self {
        MinaBaseLedgerHash0StableV1(fp.into()).into()
    }

    pub fn zero() -> Self {
        MinaBaseLedgerHash0StableV1(BigInt::zero()).into()
    }
}

impl PendingCoinbaseHash {
    pub fn from_fp(fp: Fp) -> Self {
        MinaBasePendingCoinbaseHashVersionedStableV1(MinaBasePendingCoinbaseHashBuilderStableV1(
            fp.into(),
        ))
        .into()
    }
}

impl generated::MinaStateProtocolStateBodyValueStableV2 {
    // TODO(binier): change return type to `StateBodyHash`
    pub fn try_hash(&self) -> Result<MinaBaseStateBodyHashStableV1, InvalidBigInt> {
        let fp = MinaHash::try_hash(self)?;
        Ok(MinaBaseStateBodyHashStableV1(fp.into()))
    }
}

impl generated::MinaStateProtocolStateValueStableV2 {
    pub fn try_hash(&self) -> Result<StateHash, InvalidBigInt> {
        Ok(StateHash::from_fp(MinaHash::try_hash(self)?))
    }
}

impl generated::MinaBlockHeaderStableV2 {
    pub fn try_hash(&self) -> Result<StateHash, InvalidBigInt> {
        self.protocol_state.try_hash()
    }
}

impl generated::MinaBlockBlockStableV2 {
    pub fn try_hash(&self) -> Result<StateHash, InvalidBigInt> {
        self.header.protocol_state.try_hash()
    }
}

impl MinaHash for MinaStateProtocolStateBodyValueStableV2 {
    fn try_hash(&self) -> Result<mina_hasher::Fp, InvalidBigInt> {
        let mut inputs = Inputs::new();
        self.to_input(&mut inputs)?;
        Ok(hash_with_kimchi(
            &MINA_PROTO_STATE_BODY,
            &inputs.to_fields(),
        ))
    }
}

impl MinaHash for MinaStateProtocolStateValueStableV2 {
    fn try_hash(&self) -> Result<mina_hasher::Fp, InvalidBigInt> {
        Ok(fp_state_hash_from_fp_hashes(
            self.previous_state_hash.to_field()?,
            MinaHash::try_hash(&self.body)?,
        ))
    }
}

impl FailableToInputs for MinaStateProtocolStateBodyValueStableV2 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        let MinaStateProtocolStateBodyValueStableV2 {
            genesis_state_hash,
            blockchain_state,
            consensus_state,
            constants,
        } = self;

        constants.to_input(inputs)?;
        genesis_state_hash.to_input(inputs)?;
        blockchain_state.to_input(inputs)?;
        consensus_state.to_input(inputs)?;
        Ok(())
    }
}

impl FailableToInputs for MinaBaseProtocolConstantsCheckedValueStableV1 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        let MinaBaseProtocolConstantsCheckedValueStableV1 {
            k,
            slots_per_epoch,
            slots_per_sub_window,
            grace_period_slots,
            delta,
            genesis_state_timestamp,
        } = self;

        k.to_input(inputs)?;
        delta.to_input(inputs)?;
        slots_per_epoch.to_input(inputs)?;
        slots_per_sub_window.to_input(inputs)?;
        grace_period_slots.to_input(inputs)?;
        genesis_state_timestamp.to_input(inputs)?;
        Ok(())
    }
}

impl FailableToInputs for MinaStateBlockchainStateValueStableV2 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        let MinaStateBlockchainStateValueStableV2 {
            staged_ledger_hash,
            genesis_ledger_hash,
            ledger_proof_statement,
            timestamp,
            body_reference,
        } = self;

        staged_ledger_hash.to_input(inputs)?;
        genesis_ledger_hash.to_input(inputs)?;
        ledger_proof_statement.to_input(inputs)?;
        timestamp.to_input(inputs)?;
        body_reference.to_input(inputs)?;

        Ok(())
    }
}

impl FailableToInputs for ConsensusProofOfStakeDataConsensusStateValueStableV2 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        let ConsensusProofOfStakeDataConsensusStateValueStableV2 {
            blockchain_length,
            epoch_count,
            min_window_density,
            sub_window_densities,
            last_vrf_output,
            total_currency,
            curr_global_slot_since_hard_fork,
            global_slot_since_genesis,
            staking_epoch_data,
            next_epoch_data,
            has_ancestor_in_same_checkpoint_window,
            block_stake_winner,
            block_creator,
            coinbase_receiver,
            supercharge_coinbase,
        } = self;
        blockchain_length.to_input(inputs)?;
        epoch_count.to_input(inputs)?;
        min_window_density.to_input(inputs)?;
        sub_window_densities.to_input(inputs)?;
        last_vrf_output.to_input(inputs)?;
        total_currency.to_input(inputs)?;
        curr_global_slot_since_hard_fork.to_input(inputs)?;
        global_slot_since_genesis.to_input(inputs)?;
        has_ancestor_in_same_checkpoint_window.to_input(inputs)?;
        supercharge_coinbase.to_input(inputs)?;
        staking_epoch_data.to_input(inputs)?;
        next_epoch_data.to_input(inputs)?;
        block_stake_winner.to_input(inputs)?;
        block_creator.to_input(inputs)?;
        coinbase_receiver.to_input(inputs)?;
        Ok(())
    }
}

impl FailableToInputs for MinaBaseStagedLedgerHashStableV1 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        let MinaBaseStagedLedgerHashStableV1 {
            non_snark,
            pending_coinbase_hash,
        } = self;
        non_snark.to_input(inputs)?;
        pending_coinbase_hash.to_input(inputs)?;
        Ok(())
    }
}

impl FailableToInputs for MinaBaseStagedLedgerHashNonSnarkStableV1 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        inputs.append_bytes(self.sha256().as_ref());
        Ok(())
    }
}

impl FailableToInputs for MinaStateBlockchainStateValueStableV2LedgerProofStatement {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        let MinaStateBlockchainStateValueStableV2LedgerProofStatement {
            source,
            target,
            connecting_ledger_left,
            connecting_ledger_right,
            supply_increase,
            fee_excess,
            sok_digest: _,
        } = self;
        source.to_input(inputs)?;
        target.to_input(inputs)?;
        connecting_ledger_left.to_input(inputs)?;
        connecting_ledger_right.to_input(inputs)?;
        supply_increase.to_input(inputs)?;
        fee_excess.to_input(inputs)?;
        Ok(())
    }
}

impl FailableToInputs for ConsensusBodyReferenceStableV1 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        inputs.append_bytes(self.as_ref());
        Ok(())
    }
}

impl FailableToInputs for ConsensusVrfOutputTruncatedStableV1 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        let vrf: &[u8] = self.as_ref();
        inputs.append_bytes(&vrf[..31]);
        // Ignore the last 3 bits
        let last_byte = vrf[31];
        for bit in [1, 2, 4, 8, 16] {
            inputs.append_bool(last_byte & bit != 0);
        }
        Ok(())
    }
}

impl FailableToInputs for ConsensusGlobalSlotStableV1 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        let ConsensusGlobalSlotStableV1 {
            slot_number,
            slots_per_epoch,
        } = self;
        slot_number.to_input(inputs)?;
        slots_per_epoch.to_input(inputs)?;
        Ok(())
    }
}

impl FailableToInputs for ConsensusProofOfStakeDataEpochDataStakingValueVersionedValueStableV1 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        let ConsensusProofOfStakeDataEpochDataStakingValueVersionedValueStableV1 {
            ledger,
            seed,
            start_checkpoint,
            lock_checkpoint,
            epoch_length,
        } = self;
        seed.to_input(inputs)?;
        start_checkpoint.to_input(inputs)?;
        epoch_length.to_input(inputs)?;
        ledger.to_input(inputs)?;
        lock_checkpoint.to_input(inputs)?;
        Ok(())
    }
}

impl FailableToInputs for ConsensusProofOfStakeDataEpochDataNextValueVersionedValueStableV1 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        let ConsensusProofOfStakeDataEpochDataNextValueVersionedValueStableV1 {
            ledger,
            seed,
            start_checkpoint,
            lock_checkpoint,
            epoch_length,
        } = self;
        seed.to_input(inputs)?;
        start_checkpoint.to_input(inputs)?;
        epoch_length.to_input(inputs)?;
        ledger.to_input(inputs)?;
        lock_checkpoint.to_input(inputs)?;
        Ok(())
    }
}

impl FailableToInputs for NonZeroCurvePointUncompressedStableV1 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        let NonZeroCurvePointUncompressedStableV1 { x, is_odd } = self;
        x.to_input(inputs)?;
        is_odd.to_input(inputs)?;
        Ok(())
    }
}

impl FailableToInputs for MinaStateBlockchainStateValueStableV2LedgerProofStatementSource {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        let MinaStateBlockchainStateValueStableV2LedgerProofStatementSource {
            first_pass_ledger,
            second_pass_ledger,
            pending_coinbase_stack,
            local_state,
        } = self;
        first_pass_ledger.to_input(inputs)?;
        second_pass_ledger.to_input(inputs)?;
        pending_coinbase_stack.to_input(inputs)?;
        local_state.to_input(inputs)?;
        Ok(())
    }
}

impl FailableToInputs for SignedAmount {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        let SignedAmount { magnitude, sgn } = self;
        magnitude.to_input(inputs)?;
        sgn.to_input(inputs)?;
        Ok(())
    }
}

impl FailableToInputs for MinaBaseFeeExcessStableV1 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        let MinaBaseFeeExcessStableV1(left, right) = self;
        left.to_input(inputs)?;
        right.to_input(inputs)?;
        Ok(())
    }
}

impl FailableToInputs for TokenFeeExcess {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        let TokenFeeExcess { token, amount } = self;
        token.to_input(inputs)?;
        amount.to_input(inputs)?;
        Ok(())
    }
}

impl FailableToInputs for MinaBaseEpochLedgerValueStableV1 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        let MinaBaseEpochLedgerValueStableV1 {
            hash,
            total_currency,
        } = self;
        hash.to_input(inputs)?;
        total_currency.to_input(inputs)?;
        Ok(())
    }
}

impl FailableToInputs for MinaBasePendingCoinbaseStackVersionedStableV1 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        let MinaBasePendingCoinbaseStackVersionedStableV1 { data, state } = self;
        data.to_input(inputs)?;
        state.to_input(inputs)?;
        Ok(())
    }
}

impl FailableToInputs for MinaBasePendingCoinbaseStateStackStableV1 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        let MinaBasePendingCoinbaseStateStackStableV1 { init, curr } = self;
        init.to_input(inputs)?;
        curr.to_input(inputs)?;
        Ok(())
    }
}

impl FailableToInputs for MinaTransactionLogicZkappCommandLogicLocalStateValueStableV1 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        let MinaTransactionLogicZkappCommandLogicLocalStateValueStableV1 {
            stack_frame,
            call_stack,
            transaction_commitment,
            full_transaction_commitment,
            excess,
            supply_increase,
            ledger,
            success,
            account_update_index,
            failure_status_tbl: _,
            will_succeed,
        } = self;
        stack_frame.to_input(inputs)?;
        call_stack.to_input(inputs)?;
        transaction_commitment.to_input(inputs)?;
        full_transaction_commitment.to_input(inputs)?;
        excess.to_input(inputs)?;
        supply_increase.to_input(inputs)?;
        ledger.to_input(inputs)?;
        account_update_index.to_input(inputs)?;
        success.to_input(inputs)?;
        will_succeed.to_input(inputs)?;
        Ok(())
    }
}

impl FailableToInputs for SgnStableV1 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        inputs.append_bool(self == &SgnStableV1::Pos);
        Ok(())
    }
}

impl FailableToInputs for MinaNumbersGlobalSlotSinceGenesisMStableV1 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        match self {
            MinaNumbersGlobalSlotSinceGenesisMStableV1::SinceGenesis(v) => v.to_input(inputs),
        }
    }
}

impl FailableToInputs for MinaStateBlockchainStateValueStableV2SignedAmount {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        let MinaStateBlockchainStateValueStableV2SignedAmount { magnitude, sgn } = self;
        magnitude.to_input(inputs)?;
        sgn.to_input(inputs)?;
        Ok(())
    }
}

impl FailableToInputs for MinaNumbersGlobalSlotSinceHardForkMStableV1 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        match self {
            MinaNumbersGlobalSlotSinceHardForkMStableV1::SinceHardFork(v) => v.to_input(inputs),
        }
    }
}

impl FailableToInputs for MinaNumbersGlobalSlotSpanStableV1 {
    fn to_input(&self, inputs: &mut Inputs) -> Result<(), InvalidBigInt> {
        match self {
            MinaNumbersGlobalSlotSpanStableV1::GlobalSlotSpan(v) => v.to_input(inputs),
        }
    }
}

#[cfg(test)]
mod hash_tests {
    use binprot::BinProtRead;

    use crate::v2::{
        MinaBaseZkappCommandTStableV1WireStableV1, MinaStateProtocolStateValueStableV2,
    };

    #[test]
    #[ignore = "fix expected hash/hasing"]
    fn state_hash() {
        const HASH: &str = "3NKpXp2SXWGC3XHnAJYjGtNcbq8tzossqj6kK4eGr6mSyJoFmpxR";
        const JSON: &str = include_str!("../../tests/files/v2/state/617-3NKpXp2SXWGC3XHnAJYjGtNcbq8tzossqj6kK4eGr6mSyJoFmpxR.json");

        let state: MinaStateProtocolStateValueStableV2 = serde_json::from_str(JSON).unwrap();
        let hash = state.try_hash().unwrap();
        let expected_hash = serde_json::from_value(serde_json::json!(HASH)).unwrap();
        assert_eq!(hash, expected_hash)
    }

    #[test]
    fn test_zkapp_with_proof_auth_hash() {
        // expected: 5JtkEP5AugQKKQAk3YKFxxUDggWf8AiAYyCQy49t2kLHRgPqcP8o
        // MinaBaseZkappCommandTStableV1WireStableV1
        //
        let expected_hash = "5JtkEP5AugQKKQAk3YKFxxUDggWf8AiAYyCQy49t2kLHRgPqcP8o".to_string();
        let bytes = include_bytes!("../../../tests/files/zkapps/with_proof_auth.bin");
        let zkapp =
            MinaBaseZkappCommandTStableV1WireStableV1::binprot_read(&mut bytes.as_slice()).unwrap();
        let hash = zkapp.hash().unwrap().to_string();

        assert_eq!(expected_hash, hash);
    }

    #[test]

    fn test_zkapp_with_sig_auth_hash() {
        let expected_hash = "5JvQ6xQeGgCTe2d4KpCsJ97yK61mNRZHixJxPbKTppY1qSGgtj6t".to_string();
        let bytes = include_bytes!("../../../tests/files/zkapps/with_sig_auth.bin");
        let zkapp =
            MinaBaseZkappCommandTStableV1WireStableV1::binprot_read(&mut bytes.as_slice()).unwrap();
        let hash = zkapp.hash().unwrap().to_string();

        assert_eq!(expected_hash, hash);
    }
}
