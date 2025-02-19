use super::*;

use crate::sign::basic_sig::BasicSigVerifierInternal;
use crate::sign::basic_sig::{BasicSignVerifierByPublicKeyInternal, BasicSignerInternal};
use crate::sign::multi_sig::MultiSigVerifierInternal;
use crate::sign::multi_sig::MultiSignerInternal;
use crate::sign::threshold_sig::{ThresholdSigVerifierInternal, ThresholdSignerInternal};
pub use canister_threshold_sig::ecdsa::{derive_tecdsa_public_key, get_tecdsa_master_public_key};
use ic_crypto_internal_csp::types::{CspPublicKey, CspSignature};
use ic_crypto_internal_csp::CryptoServiceProvider;
use ic_interfaces::crypto::{
    BasicSigVerifier, BasicSigVerifierByPublicKey, BasicSigner, CanisterSigVerifier,
    MultiSigVerifier, MultiSigner, Signable, ThresholdEcdsaSigVerifier, ThresholdEcdsaSigner,
    ThresholdSigVerifier, ThresholdSigVerifierByPublicKey, ThresholdSigner,
};
use ic_logger::{debug, new_logger};
use ic_types::crypto::canister_threshold_sig::error::{
    ThresholdEcdsaCombineSigSharesError, ThresholdEcdsaSignShareError,
    ThresholdEcdsaVerifyCombinedSignatureError, ThresholdEcdsaVerifySigShareError,
};
use ic_types::crypto::canister_threshold_sig::{
    ThresholdEcdsaCombinedSignature, ThresholdEcdsaSigInputs, ThresholdEcdsaSigShare,
};
use ic_types::crypto::threshold_sig::errors::threshold_sign_error::ThresholdSignError;
use ic_types::crypto::threshold_sig::ni_dkg::DkgId;
use ic_types::crypto::KeyPurpose::CommitteeSigning;
use ic_types::crypto::{
    AlgorithmId, BasicSig, BasicSigOf, CanisterSigOf, CombinedMultiSig, CombinedMultiSigOf,
    CombinedThresholdSigOf, CryptoError, CryptoResult, IndividualMultiSig, IndividualMultiSigOf,
    ThresholdSigShareOf, UserPublicKey,
};
use ic_types::{NodeId, RegistryVersion, SubnetId};
use std::collections::{BTreeMap, BTreeSet};
use std::convert::TryFrom;
pub use threshold_sig::ThresholdSigDataStore;
pub use threshold_sig::ThresholdSigDataStoreImpl;

mod basic_sig;
mod canister_sig;
mod canister_threshold_sig;
mod multi_sig;
mod threshold_sig;

pub use canister_threshold_sig::{
    get_mega_pubkey, mega_public_key_from_proto, MEGaPublicKeyFromProtoError,
    MegaKeyFromRegistryError,
};

#[cfg(test)]
mod tests;
// TODO: Remove this indirection:
pub(crate) use ic_crypto_internal_csp::imported_utilities::sign_utils as utils;
use ic_crypto_internal_logmon::metrics::MetricsDomain;
use ic_types::signature::BasicSignatureBatch;

impl<C: CryptoServiceProvider, H: Signable> BasicSigner<H> for CryptoComponentFatClient<C> {
    fn sign_basic(
        &self,
        message: &H,
        signer: NodeId,
        registry_version: RegistryVersion,
    ) -> CryptoResult<BasicSigOf<H>> {
        let logger = new_logger!(&self.logger;
            crypto.trait_name => "BasicSigner",
            crypto.method_name => "sign_basic",
            crypto.registry_version => registry_version.get(),
        );
        debug!(logger; crypto.description => "start",);
        let start_time = self.metrics.now();
        let result = BasicSignerInternal::sign_basic(
            &self.csp,
            self.registry_client.clone(),
            message,
            signer,
            registry_version,
        );
        self.metrics.observe_full_duration_seconds(
            MetricsDomain::BasicSignature,
            "sign_basic",
            start_time,
        );
        debug!(logger;
            crypto.description => "end",
            crypto.is_ok => result.is_ok(),
            crypto.error => log_err(result.as_ref().err()),
        );
        result
    }
}

impl<C: CryptoServiceProvider, H: Signable> BasicSigVerifier<H> for CryptoComponentFatClient<C> {
    fn verify_basic_sig(
        &self,
        signature: &BasicSigOf<H>,
        message: &H,
        signer: NodeId,
        registry_version: RegistryVersion,
    ) -> CryptoResult<()> {
        let logger = new_logger!(&self.logger;
            crypto.trait_name => "BasicSigVerifier",
            crypto.method_name => "verify_basic_sig",
            crypto.registry_version => registry_version.get(),
        );
        debug!(logger; crypto.description => "start",);
        let start_time = self.metrics.now();
        let result = BasicSigVerifierInternal::verify_basic_sig(
            &self.csp,
            Arc::clone(&self.registry_client),
            signature,
            message,
            signer,
            registry_version,
        );
        self.metrics.observe_full_duration_seconds(
            MetricsDomain::BasicSignature,
            "verify_basic_sig",
            start_time,
        );
        debug!(logger;
            crypto.description => "end",
            crypto.is_ok => result.is_ok(),
            crypto.error => log_err(result.as_ref().err()),
        );
        result
    }

    fn combine_basic_sig(
        &self,
        signatures: BTreeMap<NodeId, &BasicSigOf<H>>,
        registry_version: RegistryVersion,
    ) -> CryptoResult<BasicSignatureBatch<H>> {
        let logger = new_logger!(&self.logger;
            crypto.trait_name => "BasicSigVerifier",
            crypto.method_name => "combine_basic_sig",
            crypto.registry_version => registry_version.get(),
        );
        debug!(logger; crypto.description => "start",);
        let start_time = self.metrics.now();
        let result = BasicSigVerifierInternal::combine_basic_sig(signatures);
        self.metrics.observe_full_duration_seconds(
            MetricsDomain::BasicSignature,
            "combine_basic_sig",
            start_time,
        );
        debug!(logger;
            crypto.description => "end",
            crypto.is_ok => result.is_ok(),
            crypto.error => log_err(result.as_ref().err()),
        );
        result
    }

    fn verify_basic_sig_batch(
        &self,
        signature: &BasicSignatureBatch<H>,
        message: &H,
        registry_version: RegistryVersion,
    ) -> CryptoResult<()> {
        let logger = new_logger!(&self.logger;
            crypto.trait_name => "BasicSigVerifier",
            crypto.method_name => "verify_basic_sig_batch",
            crypto.registry_version => registry_version.get(),
        );
        debug!(logger; crypto.description => "start",);
        let start_time = self.metrics.now();
        let result = BasicSigVerifierInternal::verify_basic_sig_batch(
            &self.csp,
            Arc::clone(&self.registry_client),
            signature,
            message,
            registry_version,
        );
        self.metrics.observe_full_duration_seconds(
            MetricsDomain::BasicSignature,
            "verify_basic_sig_batch",
            start_time,
        );
        debug!(logger;
            crypto.description => "end",
            crypto.is_ok => result.is_ok(),
            crypto.error => log_err(result.as_ref().err()),
        );
        result
    }
}

impl<C: CryptoServiceProvider, S: Signable> BasicSigVerifierByPublicKey<S>
    for CryptoComponentFatClient<C>
{
    fn verify_basic_sig_by_public_key(
        &self,
        signature: &BasicSigOf<S>,
        signed_bytes: &S,
        public_key: &UserPublicKey,
    ) -> CryptoResult<()> {
        let logger = new_logger!(&self.logger;
            crypto.trait_name => "BasicSigVerifierByPublicBytes",
            crypto.method_name => "verify_basic_sig_by_public_key",
            crypto.signed_bytes => hex::encode(signed_bytes.as_signed_bytes()),
            crypto.public_key => format!("{}", public_key),
        );
        debug!(logger; crypto.description => "start",);
        let start_time = self.metrics.now();
        let metrics_label = format!("verify_basic_sig_by_public_key_{}", public_key.algorithm_id);
        let result = BasicSignVerifierByPublicKeyInternal::verify_basic_sig_by_public_key(
            &self.csp,
            signature,
            signed_bytes,
            public_key,
        );
        self.metrics.observe_full_duration_seconds(
            MetricsDomain::BasicSignature,
            &metrics_label,
            start_time,
        );
        debug!(logger;
            crypto.description => "end",
            crypto.is_ok => result.is_ok(),
            crypto.error => log_err(result.as_ref().err()),
        );
        result
    }
}

impl<C: CryptoServiceProvider, H: Signable> MultiSigner<H> for CryptoComponentFatClient<C> {
    fn sign_multi(
        &self,
        message: &H,
        signer: NodeId,
        registry_version: RegistryVersion,
    ) -> CryptoResult<IndividualMultiSigOf<H>> {
        let logger = new_logger!(&self.logger;
            crypto.trait_name => "MultiSigner",
            crypto.method_name => "sign_multi",
            crypto.registry_version => registry_version.get(),
        );
        debug!(logger; crypto.description => "start",);
        let start_time = self.metrics.now();
        let result = MultiSignerInternal::sign_multi(
            &self.csp,
            Arc::clone(&self.registry_client),
            message,
            signer,
            registry_version,
        );
        self.metrics.observe_full_duration_seconds(
            MetricsDomain::MultiSignature,
            "sign_multi",
            start_time,
        );
        debug!(logger;
            crypto.description => "end",
            crypto.is_ok => result.is_ok(),
            crypto.error => log_err(result.as_ref().err()),
        );
        result
    }
}

impl<C: CryptoServiceProvider, H: Signable> MultiSigVerifier<H> for CryptoComponentFatClient<C> {
    fn verify_multi_sig_individual(
        &self,
        signature: &IndividualMultiSigOf<H>,
        message: &H,
        signer: NodeId,
        registry_version: RegistryVersion,
    ) -> CryptoResult<()> {
        let logger = new_logger!(&self.logger;
            crypto.trait_name => "MultiSigner",
            crypto.method_name => "verify_multi_sig_individual",
            crypto.registry_version => registry_version.get(),
        );
        debug!(logger; crypto.description => "start",);
        let start_time = self.metrics.now();
        let result = MultiSigVerifierInternal::verify_multi_sig_individual(
            &self.csp,
            Arc::clone(&self.registry_client),
            signature,
            message,
            signer,
            registry_version,
        );
        self.metrics.observe_full_duration_seconds(
            MetricsDomain::MultiSignature,
            "verify_multi_sig_individual",
            start_time,
        );
        debug!(logger;
            crypto.description => "end",
            crypto.is_ok => result.is_ok(),
            crypto.error => log_err(result.as_ref().err()),
        );
        result
    }

    /// Combines a non-empty collection of individual signatures into a combined
    /// signature.
    fn combine_multi_sig_individuals(
        &self,
        signatures: BTreeMap<NodeId, IndividualMultiSigOf<H>>,
        registry_version: RegistryVersion,
    ) -> CryptoResult<CombinedMultiSigOf<H>> {
        let signature_count = signatures.len();
        let logger = new_logger!(&self.logger;
            crypto.trait_name => "MultiSigner",
            crypto.method_name => "combine_multi_sig_individuals",
            crypto.registry_version => registry_version.get(),
        );
        debug!(logger;
            crypto.description => format!("start; signature count: {}", signature_count),
        );
        let start_time = self.metrics.now();
        let result = MultiSigVerifierInternal::combine_multi_sig_individuals(
            &self.csp,
            Arc::clone(&self.registry_client),
            signatures,
            registry_version,
        );
        self.metrics.observe_full_duration_seconds(
            MetricsDomain::MultiSignature,
            "combine_multi_sig_individuals",
            start_time,
        );
        debug!(logger;
            crypto.description => format!("end; signature count: {}", signature_count),
            crypto.is_ok => result.is_ok(),
            crypto.error => log_err(result.as_ref().err()),
        );
        result
    }

    /// Verifies a combined signature from a non-empty set of signers. Panics if
    /// called with zero signers.
    fn verify_multi_sig_combined(
        &self,
        signature: &CombinedMultiSigOf<H>,
        message: &H,
        signers: BTreeSet<NodeId>,
        registry_version: RegistryVersion,
    ) -> CryptoResult<()> {
        let logger = new_logger!(&self.logger;
            crypto.trait_name => "MultiSigner",
            crypto.method_name => "verify_multi_sig_combined",
            crypto.registry_version => registry_version.get(),
        );
        debug!(logger;
            crypto.description => format!("start; signers: {:?}", signers),
        );
        let start_time = self.metrics.now();
        let result = MultiSigVerifierInternal::verify_multi_sig_combined(
            &self.csp,
            Arc::clone(&self.registry_client),
            signature,
            message,
            signers.clone(),
            registry_version,
        );
        self.metrics.observe_full_duration_seconds(
            MetricsDomain::MultiSignature,
            "verify_multi_sig_combined",
            start_time,
        );
        debug!(logger;
            crypto.description => format!("end; signers: {:?}", signers),
            crypto.is_ok => result.is_ok(),
            crypto.error => log_err(result.as_ref().err()),
        );
        result
    }
}

impl<C: CryptoServiceProvider, T: Signable> ThresholdSigner<T> for CryptoComponentFatClient<C> {
    // TODO (CRP-479): switch to Result<ThresholdSigShareOf<T>,
    // ThresholdSigDataNotFoundError>
    fn sign_threshold(&self, message: &T, dkg_id: DkgId) -> CryptoResult<ThresholdSigShareOf<T>> {
        let logger = new_logger!(&self.logger;
            crypto.trait_name => "ThresholdSigner",
            crypto.method_name => "sign_threshold",
            crypto.dkg_id => format!("{}", dkg_id),
        );
        debug!(logger; crypto.description => "start",);
        let start_time = self.metrics.now();
        let result = ThresholdSignerInternal::sign_threshold(
            &self.lockable_threshold_sig_data_store,
            &self.csp,
            message,
            dkg_id,
        );
        self.metrics.observe_full_duration_seconds(
            MetricsDomain::ThresholdSignature,
            "sign_threshold",
            start_time,
        );
        debug!(logger;
            crypto.description => "end",
            crypto.is_ok => result.is_ok(),
            crypto.error => log_err(result.as_ref().err()),
        );
        Ok(result?)
    }
}

impl<C: CryptoServiceProvider, T: Signable> ThresholdSigVerifier<T>
    for CryptoComponentFatClient<C>
{
    fn verify_threshold_sig_share(
        &self,
        signature: &ThresholdSigShareOf<T>,
        message: &T,
        dkg_id: DkgId,
        signer: NodeId,
    ) -> CryptoResult<()> {
        let logger = new_logger!(&self.logger;
            crypto.trait_name => "ThresholdSigVerifier",
            crypto.method_name => "verify_threshold_sig_share",
            crypto.dkg_id => format!("{}", dkg_id),
        );
        debug!(logger; crypto.description => "start",);
        let start_time = self.metrics.now();
        let result = ThresholdSigVerifierInternal::verify_threshold_sig_share(
            &self.lockable_threshold_sig_data_store,
            &self.csp,
            signature,
            message,
            dkg_id,
            signer,
        );
        self.metrics.observe_full_duration_seconds(
            MetricsDomain::ThresholdSignature,
            "verify_threshold_sig_share",
            start_time,
        );
        debug!(logger;
            crypto.description => "end",
            crypto.is_ok => result.is_ok(),
            crypto.error => log_err(result.as_ref().err()),
        );
        result
    }

    fn combine_threshold_sig_shares(
        &self,
        shares: BTreeMap<NodeId, ThresholdSigShareOf<T>>,
        dkg_id: DkgId,
    ) -> CryptoResult<CombinedThresholdSigOf<T>> {
        let nodes_with_share: BTreeSet<NodeId> = shares.keys().cloned().collect();
        let logger = new_logger!(&self.logger;
            crypto.trait_name => "ThresholdSigVerifier",
            crypto.method_name => "combine_threshold_sig_shares",
            crypto.dkg_id => format!("{}", dkg_id),
        );
        debug!(logger;
            crypto.description => format!("start; nodes with share: {:?}", nodes_with_share),
        );
        let start_time = self.metrics.now();
        let result = ThresholdSigVerifierInternal::combine_threshold_sig_shares(
            &self.lockable_threshold_sig_data_store,
            &self.csp,
            shares,
            dkg_id,
        );
        self.metrics.observe_full_duration_seconds(
            MetricsDomain::ThresholdSignature,
            "combine_threshold_sig_shares",
            start_time,
        );
        debug!(logger;
            crypto.description => format!("end; nodes with share: {:?}", nodes_with_share),
            crypto.is_ok => result.is_ok(),
            crypto.error => log_err(result.as_ref().err()),
        );
        result
    }

    fn verify_threshold_sig_combined(
        &self,
        signature: &CombinedThresholdSigOf<T>,
        message: &T,
        dkg_id: DkgId,
    ) -> CryptoResult<()> {
        let logger = new_logger!(&self.logger;
            crypto.trait_name => "ThresholdSigVerifier",
            crypto.method_name => "verify_threshold_sig_combined",
            crypto.dkg_id => format!("{}", dkg_id),
        );
        debug!(logger;
            crypto.description => "start",
        );
        let start_time = self.metrics.now();
        let result = ThresholdSigVerifierInternal::verify_threshold_sig_combined(
            &self.lockable_threshold_sig_data_store,
            &self.csp,
            signature,
            message,
            dkg_id,
        );
        self.metrics.observe_full_duration_seconds(
            MetricsDomain::ThresholdSignature,
            "verify_threshold_sig_combined",
            start_time,
        );
        debug!(logger;
            crypto.description => "end",
            crypto.is_ok => result.is_ok(),
            crypto.error => log_err(result.as_ref().err()),
        );
        result
    }
}

impl<C: CryptoServiceProvider, T: Signable> ThresholdSigVerifierByPublicKey<T>
    for CryptoComponentFatClient<C>
{
    fn verify_combined_threshold_sig_by_public_key(
        &self,
        signature: &CombinedThresholdSigOf<T>,
        message: &T,
        subnet_id: SubnetId,
        registry_version: RegistryVersion,
    ) -> CryptoResult<()> {
        let logger = new_logger!(&self.logger;
            crypto.trait_name => "ThresholdSigVerifierByPublicKey",
            crypto.method_name => "verify_combined_threshold_sig_by_public_key",
            crypto.subnet_id => format!("{}", subnet_id),
            crypto.registry_version => registry_version.get(),
        );
        debug!(logger;
            crypto.description => "start",
        );
        let start_time = self.metrics.now();
        let result = ThresholdSigVerifierInternal::verify_combined_threshold_sig_by_public_key(
            &self.csp,
            Arc::clone(&self.registry_client),
            signature,
            message,
            subnet_id,
            registry_version,
        );
        self.metrics.observe_full_duration_seconds(
            MetricsDomain::ThresholdSignature,
            "verify_combined_threshold_sig_by_public_key",
            start_time,
        );
        debug!(logger;
            crypto.description => "end",
            crypto.is_ok => result.is_ok(),
            crypto.error => log_err(result.as_ref().err()),
        );
        result
    }
}

impl<C: CryptoServiceProvider, S: Signable> CanisterSigVerifier<S> for CryptoComponentFatClient<C> {
    fn verify_canister_sig(
        &self,
        signature: &CanisterSigOf<S>,
        signed_bytes: &S,
        public_key: &UserPublicKey,
        registry_version: RegistryVersion,
    ) -> CryptoResult<()> {
        let logger = new_logger!(&self.logger;
            crypto.trait_name => "CanisterSigVerifier",
            crypto.method_name => "verify_canister_sig",
            crypto.signed_bytes => hex::encode(signed_bytes.as_signed_bytes()),
            crypto.public_key => format!("{}", public_key),
            crypto.registry_version => registry_version.get(),
        );
        debug!(logger;
            crypto.description => "start",
        );
        let start_time = self.metrics.now();
        let result = canister_sig::verify_canister_sig(
            Arc::clone(&self.registry_client),
            signature,
            signed_bytes,
            public_key,
            registry_version,
        );
        self.metrics.observe_full_duration_seconds(
            MetricsDomain::IcCanisterSignature,
            "verify_canister_sig",
            start_time,
        );
        debug!(logger;
            crypto.description => "end",
            crypto.is_ok => result.is_ok(),
            crypto.error => log_err(result.as_ref().err()),
        );
        result
    }
}

impl<C: CryptoServiceProvider> ThresholdEcdsaSigner for CryptoComponentFatClient<C> {
    fn sign_share(
        &self,
        inputs: &ThresholdEcdsaSigInputs,
    ) -> Result<ThresholdEcdsaSigShare, ThresholdEcdsaSignShareError> {
        let logger = new_logger!(&self.logger;
            crypto.trait_name => "ThresholdEcdsaSigner",
            crypto.method_name => "sign_share",
        );
        debug!(logger;
            crypto.description => "start",
        );
        let start_time = self.metrics.now();
        let result = canister_threshold_sig::ecdsa::sign_share(&self.csp, &self.node_id, inputs);
        self.metrics.observe_full_duration_seconds(
            MetricsDomain::ThresholdEcdsa,
            "sign_share",
            start_time,
        );
        debug!(logger;
            crypto.description => "end",
            crypto.is_ok => result.is_ok(),
            crypto.error => log_err(result.as_ref().err()),
        );
        result
    }
}

impl<C: CryptoServiceProvider> ThresholdEcdsaSigVerifier for CryptoComponentFatClient<C> {
    fn verify_sig_share(
        &self,
        signer: NodeId,
        inputs: &ThresholdEcdsaSigInputs,
        share: &ThresholdEcdsaSigShare,
    ) -> Result<(), ThresholdEcdsaVerifySigShareError> {
        let logger = new_logger!(&self.logger;
            crypto.trait_name => "ThresholdEcdsaSigVerifier",
            crypto.method_name => "verify_sig_share",
        );
        debug!(logger;
            crypto.description => "start",
        );
        let start_time = self.metrics.now();
        let result =
            canister_threshold_sig::ecdsa::verify_sig_share(&self.csp, signer, inputs, share);
        self.metrics.observe_full_duration_seconds(
            MetricsDomain::ThresholdEcdsa,
            "verify_sig_share",
            start_time,
        );
        debug!(logger;
            crypto.description => "end",
            crypto.is_ok => result.is_ok(),
            crypto.error => log_err(result.as_ref().err()),
        );
        result
    }

    fn combine_sig_shares(
        &self,
        inputs: &ThresholdEcdsaSigInputs,
        shares: &BTreeMap<NodeId, ThresholdEcdsaSigShare>,
    ) -> Result<ThresholdEcdsaCombinedSignature, ThresholdEcdsaCombineSigSharesError> {
        let logger = new_logger!(&self.logger;
            crypto.trait_name => "ThresholdEcdsaSigVerifier",
            crypto.method_name => "combine_sig_shares",
        );
        debug!(logger;
            crypto.description => "start",
        );
        let start_time = self.metrics.now();
        let result = canister_threshold_sig::ecdsa::combine_sig_shares(&self.csp, inputs, shares);
        self.metrics.observe_full_duration_seconds(
            MetricsDomain::ThresholdEcdsa,
            "combine_sig_shares",
            start_time,
        );
        debug!(logger;
            crypto.description => "end",
            crypto.is_ok => result.is_ok(),
            crypto.error => log_err(result.as_ref().err()),
        );
        result
    }

    fn verify_combined_sig(
        &self,
        inputs: &ThresholdEcdsaSigInputs,
        signature: &ThresholdEcdsaCombinedSignature,
    ) -> Result<(), ThresholdEcdsaVerifyCombinedSignatureError> {
        let logger = new_logger!(&self.logger;
            crypto.trait_name => "ThresholdEcdsaSigVerifier",
            crypto.method_name => "verify_combined_sig",
        );
        debug!(logger;
            crypto.description => "start",
        );
        let start_time = self.metrics.now();
        let result =
            canister_threshold_sig::ecdsa::verify_combined_signature(&self.csp, inputs, signature);
        self.metrics.observe_full_duration_seconds(
            MetricsDomain::ThresholdEcdsa,
            "verify_combined_sig",
            start_time,
        );
        debug!(logger;
            crypto.description => "end",
            crypto.is_ok => result.is_ok(),
            crypto.error => log_err(result.as_ref().err()),
        );
        result
    }
}

fn log_err<T: fmt::Display>(error_option: Option<&T>) -> String {
    if let Some(error) = error_option {
        return format!("{}", error);
    }
    "none".to_string()
}

pub fn log_ok_content<T: fmt::Display, E>(result: &Result<T, E>) -> String {
    if let Ok(content) = result {
        return format!("{}", content);
    }
    "none".to_string()
}
