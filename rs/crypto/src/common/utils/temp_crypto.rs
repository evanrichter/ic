use crate::common::utils::generate_tls_keys;
use crate::common::utils::{
    generate_committee_signing_keys, generate_dkg_dealing_encryption_keys,
    generate_idkg_dealing_encryption_keys, generate_node_signing_keys,
};
use crate::utils::csp_for_config;
use crate::{derive_node_id, CryptoComponent, CryptoComponentFatClient};
use async_trait::async_trait;
use ic_base_types::PrincipalId;
use ic_config::crypto::{CryptoConfig, CspVaultType};
use ic_crypto_internal_csp::secret_key_store::proto_store::ProtoSecretKeyStore;
use ic_crypto_internal_csp::secret_key_store::volatile_store::VolatileSecretKeyStore;
use ic_crypto_internal_csp::vault::remote_csp_vault::TarpcCspVaultServerImpl;
use ic_crypto_internal_csp::{public_key_store, CryptoServiceProvider, Csp};
use ic_crypto_internal_logmon::metrics::CryptoMetrics;
use ic_crypto_internal_seed::Seed;
use ic_crypto_tls_interfaces::TlsPublicKeyCert;
use ic_crypto_tls_interfaces::{
    AllowedClients, AuthenticatedPeer, TlsClientHandshakeError, TlsHandshake,
    TlsServerHandshakeError, TlsStream,
};
use ic_interfaces::crypto::{
    BasicSigVerifier, BasicSigVerifierByPublicKey, CanisterSigVerifier, IDkgProtocol, KeyManager,
    MultiSigVerifier, Signable, ThresholdEcdsaSigVerifier, ThresholdEcdsaSigner,
    ThresholdSigVerifier, ThresholdSigVerifierByPublicKey,
};
use ic_interfaces::registry::RegistryClient;
use ic_logger::replica_logger::no_op_logger;
use ic_logger::ReplicaLogger;
use ic_protobuf::crypto::v1::NodePublicKeys;
use ic_protobuf::registry::crypto::v1::PublicKey as PublicKeyProto;
use ic_registry_client_fake::FakeRegistryClient;
use ic_registry_keys::{make_crypto_node_key, make_crypto_tls_cert_key};
use ic_registry_proto_data_provider::ProtoRegistryDataProvider;
use ic_types::crypto::canister_threshold_sig::error::{
    IDkgCreateDealingError, IDkgCreateTranscriptError, IDkgLoadTranscriptError,
    IDkgOpenTranscriptError, IDkgRetainThresholdKeysError, IDkgVerifyComplaintError,
    IDkgVerifyDealingPrivateError, IDkgVerifyDealingPublicError, IDkgVerifyOpeningError,
    IDkgVerifyTranscriptError, ThresholdEcdsaCombineSigSharesError, ThresholdEcdsaSignShareError,
    ThresholdEcdsaVerifyCombinedSignatureError, ThresholdEcdsaVerifySigShareError,
};
use ic_types::crypto::canister_threshold_sig::idkg::{
    BatchSignedIDkgDealing, IDkgComplaint, IDkgDealing, IDkgOpening, IDkgTranscript,
    IDkgTranscriptParams,
};
use ic_types::crypto::canister_threshold_sig::{
    ThresholdEcdsaCombinedSignature, ThresholdEcdsaSigInputs, ThresholdEcdsaSigShare,
};
use ic_types::crypto::threshold_sig::ni_dkg::DkgId;
use ic_types::crypto::{
    BasicSigOf, CanisterSigOf, CombinedMultiSigOf, CombinedThresholdSigOf, CryptoResult,
    IndividualMultiSigOf, KeyPurpose, ThresholdSigShareOf, UserPublicKey,
};
use ic_types::signature::BasicSignatureBatch;
use ic_types::{NodeId, RegistryVersion, SubnetId};
use rand::rngs::OsRng;
use rand_chacha::ChaChaRng;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::net::{TcpStream, UnixListener};

#[cfg(test)]
mod tests;

/// A crypto component set up in a temporary directory. The directory is
/// automatically deleted when this component goes out of scope.
pub type TempCryptoComponent =
    TempCryptoComponentGeneric<Csp<OsRng, ProtoSecretKeyStore, ProtoSecretKeyStore>>;

/// This struct combines the following two items:
/// * a crypto component whose state lives in a temporary directory
/// * a newly created temporary directory that contains the state
///
/// Combining these two items is useful for testing because the temporary
/// directory will exist for as long as the struct exists and is automatically
/// deleted once the struct goes out of scope.
pub struct TempCryptoComponentGeneric<C: CryptoServiceProvider> {
    crypto_component: CryptoComponentFatClient<C>,
    remote_vault_environment: Option<RemoteVaultEnvironment>,
    temp_dir: TempDir,
}

struct RemoteVaultEnvironment {
    vault_server: Arc<TempCspVaultServer>,
    vault_client_runtime: TokioRuntimeOrHandle,
}

enum TokioRuntimeOrHandle {
    Runtime(tokio::runtime::Runtime),
    Handle(tokio::runtime::Handle),
}

impl TokioRuntimeOrHandle {
    fn new(option_handle: Option<tokio::runtime::Handle>) -> Self {
        if let Some(handle) = option_handle {
            Self::Handle(handle)
        } else {
            let multi_thread_rt = tokio::runtime::Runtime::new().expect("failed to create runtime");
            Self::Runtime(multi_thread_rt)
        }
    }

    fn handle(&self) -> &tokio::runtime::Handle {
        match &self {
            TokioRuntimeOrHandle::Runtime(runtime) => runtime.handle(),
            TokioRuntimeOrHandle::Handle(handle) => handle,
        }
    }
}

impl<C: CryptoServiceProvider> Deref for TempCryptoComponentGeneric<C> {
    type Target = CryptoComponentFatClient<C>;

    fn deref(&self) -> &Self::Target {
        &self.crypto_component
    }
}

pub struct TempCryptoBuilder {
    node_keys_to_generate: Option<NodeKeysToGenerate>,
    registry_client: Option<Arc<dyn RegistryClient>>,
    registry_data: Option<Arc<ProtoRegistryDataProvider>>,
    registry_version: Option<RegistryVersion>,
    node_id: Option<NodeId>,
    start_remote_vault: bool,
    vault_server_runtime_handle: Option<tokio::runtime::Handle>,
    vault_client_runtime_handle: Option<tokio::runtime::Handle>,
    connected_remote_vault: Option<Arc<TempCspVaultServer>>,
    logger: Option<ReplicaLogger>,
}

impl TempCryptoBuilder {
    const DEFAULT_NODE_ID: u64 = 1;
    const DEFAULT_REGISTRY_VERSION: u64 = 1;

    pub fn with_node_id(mut self, node_id: NodeId) -> Self {
        self.node_id = Some(node_id);
        self
    }

    pub fn with_registry(mut self, registry_client: Arc<dyn RegistryClient>) -> Self {
        self.registry_client = Some(registry_client);
        self
    }

    /// If the `registry_client` is of type `FakeRegistryClient`, the caller may manually have to
    /// call `registry_client.reload()` after calling `build()`.
    pub fn with_registry_client_and_data(
        mut self,
        registry_client: Arc<dyn RegistryClient>,
        registry_data: Arc<ProtoRegistryDataProvider>,
    ) -> Self {
        self.registry_client = Some(registry_client);
        self.registry_data = Some(registry_data);
        self
    }

    pub fn with_logger(mut self, logger: ReplicaLogger) -> Self {
        self.logger = Some(logger);
        self
    }

    pub fn with_keys(mut self, keys: NodeKeysToGenerate) -> Self {
        self.node_keys_to_generate = Some(keys);
        self
    }

    pub fn with_keys_in_registry_version(
        mut self,
        keys: NodeKeysToGenerate,
        registry_version: RegistryVersion,
    ) -> Self {
        self.node_keys_to_generate = Some(keys);
        self.registry_version = Some(registry_version);
        self
    }

    pub fn with_remote_vault(mut self) -> Self {
        self.start_remote_vault = true;
        self.connected_remote_vault = None;
        self
    }

    pub fn with_vault_client_runtime(mut self, rt_handle: tokio::runtime::Handle) -> Self {
        self.vault_client_runtime_handle = Some(rt_handle);
        self
    }

    pub fn with_vault_server_runtime(mut self, rt_handle: tokio::runtime::Handle) -> Self {
        self.vault_server_runtime_handle = Some(rt_handle);
        self
    }

    pub fn with_existing_remote_vault(mut self, vault_server: Arc<TempCspVaultServer>) -> Self {
        self.connected_remote_vault = Some(vault_server);
        self.start_remote_vault = false;
        self
    }

    pub fn build(self) -> TempCryptoComponent {
        let (mut config, temp_dir) = CryptoConfig::new_in_temp_dir();
        let mut csp = csp_for_config(&config, None);
        let node_keys_to_generate = self
            .node_keys_to_generate
            .unwrap_or_else(|| NodeKeysToGenerate::none());
        let node_signing_pk = node_keys_to_generate
            .generate_node_signing_keys
            .then(|| generate_node_signing_keys(&csp));
        let node_id = self.node_id.unwrap_or_else(|| {
            node_signing_pk.as_ref().map_or(
                NodeId::from(PrincipalId::new_node_test_id(Self::DEFAULT_NODE_ID)),
                |nspk| derive_node_id(nspk),
            )
        });
        let committee_signing_pk = node_keys_to_generate
            .generate_committee_signing_keys
            .then(|| generate_committee_signing_keys(&csp));
        let dkg_dealing_encryption_pk = node_keys_to_generate
            .generate_dkg_dealing_encryption_keys
            .then(|| generate_dkg_dealing_encryption_keys(&mut csp, node_id));
        let idkg_dealing_encryption_pk = node_keys_to_generate
            .generate_idkg_dealing_encryption_keys
            .then(|| generate_idkg_dealing_encryption_keys(&mut csp));
        let tls_certificate = node_keys_to_generate
            .generate_tls_keys_and_certificate
            .then(|| generate_tls_keys(&mut csp, node_id).to_proto());

        let is_registry_data_provided = self.registry_data.is_some();
        let registry_data = self
            .registry_data
            .unwrap_or_else(|| Arc::new(ProtoRegistryDataProvider::new()));
        if is_registry_data_provided || self.registry_client.is_none() {
            // add data. if a registry_client is provided, the caller has to reload it themselves.
            let registry_version = self
                .registry_version
                .unwrap_or(RegistryVersion::new(Self::DEFAULT_REGISTRY_VERSION));
            if let Some(node_signing_pk) = &node_signing_pk {
                registry_data
                    .add(
                        &make_crypto_node_key(node_id, KeyPurpose::NodeSigning),
                        registry_version,
                        Some(node_signing_pk.to_owned()),
                    )
                    .expect("failed to add node signing key to registry");
            }
            if let Some(committee_signing_pk) = &committee_signing_pk {
                registry_data
                    .add(
                        &make_crypto_node_key(node_id, KeyPurpose::CommitteeSigning),
                        registry_version,
                        Some(committee_signing_pk.to_owned()),
                    )
                    .expect("failed to add committee signing key to registry");
            }
            if let Some(dkg_dealing_encryption_pk) = &dkg_dealing_encryption_pk {
                registry_data
                    .add(
                        &make_crypto_node_key(node_id, KeyPurpose::DkgDealingEncryption),
                        registry_version,
                        Some(dkg_dealing_encryption_pk.to_owned()),
                    )
                    .expect("failed to add NI-DKG dealing encryption key to registry");
            }
            if let Some(idkg_dealing_encryption_pk) = &idkg_dealing_encryption_pk {
                registry_data
                    .add(
                        &make_crypto_node_key(node_id, KeyPurpose::IDkgMEGaEncryption),
                        registry_version,
                        Some(idkg_dealing_encryption_pk.to_owned()),
                    )
                    .expect("failed to add iDKG dealing encryption key to registry");
            }
            if let Some(tls_certificate) = &tls_certificate {
                registry_data
                    .add(
                        &make_crypto_tls_cert_key(node_id),
                        registry_version,
                        Some(tls_certificate.to_owned()),
                    )
                    .expect("failed to add TLS certificate to registry");
            }
        }
        let registry_client = self.registry_client.unwrap_or_else(|| {
            let fake_registry_client = Arc::new(FakeRegistryClient::new(registry_data));
            fake_registry_client.reload();
            fake_registry_client as Arc<dyn RegistryClient>
        });

        let node_pubkeys = NodePublicKeys {
            version: 1,
            node_signing_pk,
            committee_signing_pk,
            dkg_dealing_encryption_pk,
            idkg_dealing_encryption_pk,
            tls_certificate,
        };
        public_key_store::store_node_public_keys(&config.crypto_root, &node_pubkeys)
            .unwrap_or_else(|_| panic!("failed to store public key material"));

        let opt_remote_vault_environment = if self.start_remote_vault {
            let vault_server =
                TempCspVaultServer::start(&config.crypto_root, self.vault_server_runtime_handle);
            config.csp_vault_type = CspVaultType::UnixSocket(vault_server.vault_socket_path());
            Some(RemoteVaultEnvironment {
                vault_server: Arc::new(vault_server),
                vault_client_runtime: TokioRuntimeOrHandle::new(self.vault_client_runtime_handle),
            })
        } else if let Some(vault_server) = self.connected_remote_vault {
            config.csp_vault_type = CspVaultType::UnixSocket(vault_server.vault_socket_path());
            Some(RemoteVaultEnvironment {
                vault_server,
                vault_client_runtime: TokioRuntimeOrHandle::new(self.vault_client_runtime_handle),
            })
        } else {
            None
        };

        let opt_vault_client_runtime_handle = opt_remote_vault_environment
            .as_ref()
            .map(|data| data.vault_client_runtime.handle().clone());
        let logger = self.logger.unwrap_or_else(|| no_op_logger());
        let crypto_component = CryptoComponent::new_with_fake_node_id(
            &config,
            opt_vault_client_runtime_handle,
            registry_client,
            node_id,
            logger,
        );

        TempCryptoComponent {
            crypto_component,
            remote_vault_environment: opt_remote_vault_environment,
            temp_dir,
        }
    }

    pub fn build_arc(self) -> Arc<TempCryptoComponent> {
        Arc::new(self.build())
    }
}

/// A struct combining a temporary directory with a CSP vault server that is
/// listening on a unix socket that lives in this temporary directory. The
/// directory is automatically deleted when this struct goes out of scope.
pub struct TempCspVaultServer {
    tokio_runtime: TokioRuntimeOrHandle,
    join_handle: tokio::task::JoinHandle<()>,
    temp_dir: TempDir,
}

impl Drop for TempCspVaultServer {
    fn drop(&mut self) {
        // Aborts the tokio task that runs the vault server. This drops
        // the server and thus the unix listener and thus the server-side
        // handle to the file acting as unix domain socket used for
        // communication with the server.
        // If also all client-side handles to the socket file are dropped,
        // then nothing should prevent the deletion (=cleanup) of the
        // directory behind `temp_dir` when `temp_dir` is dropped.
        // Note that [the fields of a struct are dropped in declaration order]
        // (https://doc.rust-lang.org/reference/destructors.html#destructors).
        self.join_handle.abort();
    }
}

impl TempCspVaultServer {
    pub fn start(crypto_root: &Path, opt_tokio_rt_handle: Option<tokio::runtime::Handle>) -> Self {
        let temp_dir = tempfile::Builder::new()
            .prefix("ic_crypto_csp_vault_")
            .tempdir()
            .expect("failed to create temporary directory");
        let vault_socket_path = Self::vault_socket_path_in(temp_dir.path());
        let tokio_runtime = TokioRuntimeOrHandle::new(opt_tokio_rt_handle);
        let listener = {
            let _enter_guard = tokio_runtime.handle().enter();
            UnixListener::bind(&vault_socket_path).expect("failed to bind")
        };
        let server = TarpcCspVaultServerImpl::new(
            crypto_root,
            listener,
            no_op_logger(),
            Arc::new(CryptoMetrics::none()),
        );
        let join_handle = tokio_runtime.handle().spawn(server.run());

        Self {
            tokio_runtime,
            join_handle,
            temp_dir,
        }
    }

    pub fn vault_socket_path(&self) -> PathBuf {
        Self::vault_socket_path_in(self.temp_dir.path())
    }

    pub fn tokio_runtime_handle(&self) -> &tokio::runtime::Handle {
        self.tokio_runtime.handle()
    }

    fn vault_socket_path_in(directory: &Path) -> PathBuf {
        let mut path = directory.to_path_buf();
        path.push("ic-crypto-csp.socket");
        path
    }
}

impl TempCryptoComponent {
    pub fn builder() -> TempCryptoBuilder {
        TempCryptoBuilder {
            node_id: None,
            start_remote_vault: false,
            vault_server_runtime_handle: None,
            vault_client_runtime_handle: None,
            registry_client: None,
            registry_data: None,
            node_keys_to_generate: None,
            registry_version: None,
            connected_remote_vault: None,
            logger: None,
        }
    }

    pub fn new(registry_client: Arc<dyn RegistryClient>, node_id: NodeId) -> Self {
        TempCryptoComponent::builder()
            .with_registry(registry_client)
            .with_node_id(node_id)
            .build()
    }

    pub fn new_with_ni_dkg_dealing_encryption_key_generation(
        registry_client: Arc<dyn RegistryClient>,
        node_id: NodeId,
    ) -> (Self, PublicKeyProto) {
        let temp_crypto = TempCryptoComponent::builder()
            .with_registry(registry_client)
            .with_node_id(node_id)
            .with_keys(NodeKeysToGenerate::only_dkg_dealing_encryption_key())
            .build();
        let dkg_dealing_encryption_pubkey = temp_crypto
            .node_public_keys()
            .dkg_dealing_encryption_pk
            .expect("missing dkg_dealing_encryption_pk");
        (temp_crypto, dkg_dealing_encryption_pubkey)
    }

    // TODO (CRP-1275): Remove this once MEGa key is in NodePublicKeys
    pub fn new_with_signing_idkg_dealing_encryption_and_multisigning_keys_generation(
        registry_client: Arc<dyn RegistryClient>,
        node_id: NodeId,
    ) -> (Self, IDkgMEGaAndMultisignPublicKeys) {
        let temp_crypto = TempCryptoComponent::builder()
            .with_registry(registry_client)
            .with_node_id(node_id)
            .with_keys(NodeKeysToGenerate {
                generate_node_signing_keys: true,
                generate_committee_signing_keys: true,
                generate_idkg_dealing_encryption_keys: true,
                ..NodeKeysToGenerate::none()
            })
            .build();
        let node_public_keys = temp_crypto.node_public_keys();

        let node_signing_pk = node_public_keys
            .node_signing_pk
            .expect("missing node_signing_pk");
        let committee_signing_pk = node_public_keys
            .committee_signing_pk
            .expect("missing committee_signing_pk");
        let idkg_dealing_encryption_pk = node_public_keys
            .idkg_dealing_encryption_pk
            .expect("missing idkg_dealing_encryption_pk");
        (
            temp_crypto,
            IDkgMEGaAndMultisignPublicKeys {
                node_signing_pubkey: node_signing_pk,
                mega_pubkey: idkg_dealing_encryption_pk,
                multisign_pubkey: committee_signing_pk,
            },
        )
    }

    pub fn new_with_tls_key_generation(
        registry_client: Arc<dyn RegistryClient>,
        node_id: NodeId,
    ) -> (Self, TlsPublicKeyCert) {
        let temp_crypto = TempCryptoComponent::builder()
            .with_registry(registry_client)
            .with_node_id(node_id)
            .with_keys(NodeKeysToGenerate::only_tls_key_and_cert())
            .build();
        let tls_certificate = temp_crypto
            .node_public_keys()
            .tls_certificate
            .expect("missing tls_certificate");
        let tls_pubkey = TlsPublicKeyCert::new_from_der(tls_certificate.certificate_der)
            .expect("failed to create X509 cert from DER");
        (temp_crypto, tls_pubkey)
    }

    pub fn new_with_node_keys_generation(
        registry_client: Arc<dyn RegistryClient>,
        node_id: NodeId,
        selector: NodeKeysToGenerate,
    ) -> (Self, NodePublicKeys) {
        let temp_crypto = TempCryptoComponent::builder()
            .with_registry(registry_client)
            .with_node_id(node_id)
            .with_keys(selector)
            .build();
        let node_pubkeys = temp_crypto.node_public_keys();
        (temp_crypto, node_pubkeys)
    }

    pub fn new_with(
        registry_client: Arc<dyn RegistryClient>,
        node_id: NodeId,
        config: &CryptoConfig,
        temp_dir: TempDir,
    ) -> Self {
        let crypto_component = CryptoComponent::new_with_fake_node_id(
            config,
            None,
            registry_client,
            node_id,
            no_op_logger(),
        );

        TempCryptoComponent {
            crypto_component,
            remote_vault_environment: None,
            temp_dir,
        }
    }

    pub fn multiple_new(
        nodes: &[NodeId],
        registry: Arc<dyn RegistryClient>,
    ) -> BTreeMap<NodeId, TempCryptoComponent> {
        nodes
            .iter()
            .map(|node| {
                let temp_crypto = Self::new(Arc::clone(&registry), *node);
                (*node, temp_crypto)
            })
            .collect()
    }

    pub fn temp_dir_path(&self) -> &std::path::Path {
        self.temp_dir.path()
    }

    pub fn vault_server(&self) -> Option<Arc<TempCspVaultServer>> {
        self.remote_vault_environment
            .as_ref()
            .map(|env| Arc::clone(&env.vault_server))
    }

    pub fn vault_client_runtime(&self) -> Option<&tokio::runtime::Handle> {
        self.remote_vault_environment
            .as_ref()
            .map(|env| env.vault_client_runtime.handle())
    }
}

/// Bundles the public keys needed for canister threshold signature protocol
// TODO (CRP-1275): Remove this once MEGa key is in NodePublicKeys
pub struct IDkgMEGaAndMultisignPublicKeys {
    pub node_signing_pubkey: PublicKeyProto,
    pub mega_pubkey: PublicKeyProto,
    pub multisign_pubkey: PublicKeyProto,
}

/// Selects which keys should be generated for a `TempCryptoComponent`.
#[derive(Clone)]
pub struct NodeKeysToGenerate {
    pub generate_node_signing_keys: bool,
    pub generate_committee_signing_keys: bool,
    pub generate_dkg_dealing_encryption_keys: bool,
    pub generate_idkg_dealing_encryption_keys: bool,
    pub generate_tls_keys_and_certificate: bool,
}

impl NodeKeysToGenerate {
    pub fn all() -> Self {
        NodeKeysToGenerate {
            generate_node_signing_keys: true,
            generate_committee_signing_keys: true,
            generate_dkg_dealing_encryption_keys: true,
            generate_idkg_dealing_encryption_keys: true,
            generate_tls_keys_and_certificate: true,
        }
    }

    pub fn none() -> Self {
        NodeKeysToGenerate {
            generate_node_signing_keys: false,
            generate_committee_signing_keys: false,
            generate_dkg_dealing_encryption_keys: false,
            generate_idkg_dealing_encryption_keys: false,
            generate_tls_keys_and_certificate: false,
        }
    }

    pub fn all_except_dkg_dealing_encryption_key() -> Self {
        NodeKeysToGenerate {
            generate_dkg_dealing_encryption_keys: false,
            ..Self::all()
        }
    }

    pub fn all_except_idkg_dealing_encryption_key() -> Self {
        NodeKeysToGenerate {
            generate_idkg_dealing_encryption_keys: false,
            ..Self::all()
        }
    }

    pub fn only_node_signing_key() -> Self {
        NodeKeysToGenerate {
            generate_node_signing_keys: true,
            ..Self::none()
        }
    }

    pub fn only_committee_signing_key() -> Self {
        NodeKeysToGenerate {
            generate_committee_signing_keys: true,
            ..Self::none()
        }
    }

    pub fn only_dkg_dealing_encryption_key() -> Self {
        NodeKeysToGenerate {
            generate_dkg_dealing_encryption_keys: true,
            ..Self::none()
        }
    }

    pub fn only_idkg_dealing_encryption_key() -> Self {
        NodeKeysToGenerate {
            generate_idkg_dealing_encryption_keys: true,
            ..Self::none()
        }
    }

    pub fn only_tls_key_and_cert() -> Self {
        NodeKeysToGenerate {
            generate_tls_keys_and_certificate: true,
            ..Self::none()
        }
    }
}

impl TempCryptoComponentGeneric<Csp<ChaChaRng, ProtoSecretKeyStore, VolatileSecretKeyStore>> {
    pub fn new_from_seed(
        seed: Seed,
        registry_client: Arc<dyn RegistryClient>,
        node_id: NodeId,
    ) -> Self {
        let (config, temp_dir) = CryptoConfig::new_in_temp_dir();
        let csprng = seed.into_rng();
        let crypto_component = CryptoComponentFatClient::new_with_rng_and_fake_node_id(
            csprng,
            &config,
            no_op_logger(),
            registry_client,
            node_id,
        );

        TempCryptoComponentGeneric {
            crypto_component,
            remote_vault_environment: None,
            temp_dir,
        }
    }
}

impl<C: CryptoServiceProvider, T: Signable> BasicSigVerifierByPublicKey<T>
    for TempCryptoComponentGeneric<C>
{
    fn verify_basic_sig_by_public_key(
        &self,
        signature: &BasicSigOf<T>,
        signed_bytes: &T,
        public_key: &UserPublicKey,
    ) -> CryptoResult<()> {
        self.crypto_component
            .verify_basic_sig_by_public_key(signature, signed_bytes, public_key)
    }
}

impl<C: CryptoServiceProvider, T: Signable> CanisterSigVerifier<T>
    for TempCryptoComponentGeneric<C>
{
    fn verify_canister_sig(
        &self,
        signature: &CanisterSigOf<T>,
        signed_bytes: &T,
        public_key: &UserPublicKey,
        registry_version: RegistryVersion,
    ) -> CryptoResult<()> {
        self.crypto_component.verify_canister_sig(
            signature,
            signed_bytes,
            public_key,
            registry_version,
        )
    }
}

impl<C: CryptoServiceProvider> IDkgProtocol for TempCryptoComponentGeneric<C> {
    fn create_dealing(
        &self,
        params: &IDkgTranscriptParams,
    ) -> Result<IDkgDealing, IDkgCreateDealingError> {
        self.crypto_component.create_dealing(params)
    }

    fn verify_dealing_public(
        &self,
        params: &IDkgTranscriptParams,
        dealing_id: NodeId,
        dealing: &IDkgDealing,
    ) -> Result<(), IDkgVerifyDealingPublicError> {
        self.crypto_component
            .verify_dealing_public(params, dealing_id, dealing)
    }

    fn verify_dealing_private(
        &self,
        params: &IDkgTranscriptParams,
        dealer_id: NodeId,
        dealing: &IDkgDealing,
    ) -> Result<(), IDkgVerifyDealingPrivateError> {
        self.crypto_component
            .verify_dealing_private(params, dealer_id, dealing)
    }

    fn create_transcript(
        &self,
        params: &IDkgTranscriptParams,
        dealings: &BTreeMap<NodeId, BatchSignedIDkgDealing>,
    ) -> Result<IDkgTranscript, IDkgCreateTranscriptError> {
        self.crypto_component.create_transcript(params, dealings)
    }

    fn verify_transcript(
        &self,
        params: &IDkgTranscriptParams,
        transcript: &IDkgTranscript,
    ) -> Result<(), IDkgVerifyTranscriptError> {
        self.crypto_component.verify_transcript(params, transcript)
    }

    fn load_transcript(
        &self,
        transcript: &IDkgTranscript,
    ) -> Result<Vec<IDkgComplaint>, IDkgLoadTranscriptError> {
        self.crypto_component.load_transcript(transcript)
    }

    fn verify_complaint(
        &self,
        transcript: &IDkgTranscript,
        complainer_id: NodeId,
        complaint: &IDkgComplaint,
    ) -> Result<(), IDkgVerifyComplaintError> {
        self.crypto_component
            .verify_complaint(transcript, complainer_id, complaint)
    }

    fn open_transcript(
        &self,
        transcript: &IDkgTranscript,
        complainer_id: NodeId,
        complaint: &IDkgComplaint,
    ) -> Result<IDkgOpening, IDkgOpenTranscriptError> {
        self.crypto_component
            .open_transcript(transcript, complainer_id, complaint)
    }

    fn verify_opening(
        &self,
        transcript: &IDkgTranscript,
        opener: NodeId,
        opening: &IDkgOpening,
        complaint: &IDkgComplaint,
    ) -> Result<(), IDkgVerifyOpeningError> {
        self.crypto_component
            .verify_opening(transcript, opener, opening, complaint)
    }

    fn load_transcript_with_openings(
        &self,
        transcript: &IDkgTranscript,
        openings: &BTreeMap<IDkgComplaint, BTreeMap<NodeId, IDkgOpening>>,
    ) -> Result<(), IDkgLoadTranscriptError> {
        self.crypto_component
            .load_transcript_with_openings(transcript, openings)
    }

    fn retain_active_transcripts(
        &self,
        active_transcripts: &BTreeSet<IDkgTranscript>,
    ) -> Result<(), IDkgRetainThresholdKeysError> {
        self.crypto_component
            .retain_active_transcripts(active_transcripts)
    }
}

impl<C: CryptoServiceProvider> ThresholdEcdsaSigner for TempCryptoComponentGeneric<C> {
    fn sign_share(
        &self,
        inputs: &ThresholdEcdsaSigInputs,
    ) -> Result<ThresholdEcdsaSigShare, ThresholdEcdsaSignShareError> {
        self.crypto_component.sign_share(inputs)
    }
}

impl<C: CryptoServiceProvider> ThresholdEcdsaSigVerifier for TempCryptoComponentGeneric<C> {
    fn verify_sig_share(
        &self,
        signer: NodeId,
        inputs: &ThresholdEcdsaSigInputs,
        share: &ThresholdEcdsaSigShare,
    ) -> Result<(), ThresholdEcdsaVerifySigShareError> {
        self.crypto_component
            .verify_sig_share(signer, inputs, share)
    }

    fn combine_sig_shares(
        &self,
        inputs: &ThresholdEcdsaSigInputs,
        shares: &BTreeMap<NodeId, ThresholdEcdsaSigShare>,
    ) -> Result<ThresholdEcdsaCombinedSignature, ThresholdEcdsaCombineSigSharesError> {
        self.crypto_component.combine_sig_shares(inputs, shares)
    }

    fn verify_combined_sig(
        &self,
        inputs: &ThresholdEcdsaSigInputs,
        signature: &ThresholdEcdsaCombinedSignature,
    ) -> Result<(), ThresholdEcdsaVerifyCombinedSignatureError> {
        self.crypto_component.verify_combined_sig(inputs, signature)
    }
}

#[async_trait]
impl<C: CryptoServiceProvider + Send + Sync> TlsHandshake for TempCryptoComponentGeneric<C> {
    async fn perform_tls_server_handshake(
        &self,
        tcp_stream: TcpStream,
        allowed_clients: AllowedClients,
        registry_version: RegistryVersion,
    ) -> Result<(TlsStream, AuthenticatedPeer), TlsServerHandshakeError> {
        self.crypto_component
            .perform_tls_server_handshake(tcp_stream, allowed_clients, registry_version)
            .await
    }

    async fn perform_tls_server_handshake_with_rustls(
        &self,
        tcp_stream: TcpStream,
        allowed_clients: AllowedClients,
        registry_version: RegistryVersion,
    ) -> Result<(TlsStream, AuthenticatedPeer), TlsServerHandshakeError> {
        self.crypto_component
            .perform_tls_server_handshake_with_rustls(tcp_stream, allowed_clients, registry_version)
            .await
    }

    async fn perform_tls_server_handshake_without_client_auth(
        &self,
        tcp_stream: TcpStream,
        registry_version: RegistryVersion,
    ) -> Result<TlsStream, TlsServerHandshakeError> {
        self.crypto_component
            .perform_tls_server_handshake_without_client_auth(tcp_stream, registry_version)
            .await
    }

    async fn perform_tls_server_handshake_without_client_auth_with_rustls(
        &self,
        tcp_stream: TcpStream,
        registry_version: RegistryVersion,
    ) -> Result<TlsStream, TlsServerHandshakeError> {
        self.crypto_component
            .perform_tls_server_handshake_without_client_auth_with_rustls(
                tcp_stream,
                registry_version,
            )
            .await
    }

    async fn perform_tls_client_handshake(
        &self,
        tcp_stream: TcpStream,
        server: NodeId,
        registry_version: RegistryVersion,
    ) -> Result<TlsStream, TlsClientHandshakeError> {
        self.crypto_component
            .perform_tls_client_handshake(tcp_stream, server, registry_version)
            .await
    }

    async fn perform_tls_client_handshake_with_rustls(
        &self,
        tcp_stream: TcpStream,
        server: NodeId,
        registry_version: RegistryVersion,
    ) -> Result<TlsStream, TlsClientHandshakeError> {
        self.crypto_component
            .perform_tls_client_handshake_with_rustls(tcp_stream, server, registry_version)
            .await
    }
}

impl<C: CryptoServiceProvider, T: Signable> BasicSigVerifier<T> for TempCryptoComponentGeneric<C> {
    fn verify_basic_sig(
        &self,
        signature: &BasicSigOf<T>,
        message: &T,
        signer: NodeId,
        registry_version: RegistryVersion,
    ) -> CryptoResult<()> {
        self.crypto_component
            .verify_basic_sig(signature, message, signer, registry_version)
    }

    fn combine_basic_sig(
        &self,
        signatures: BTreeMap<NodeId, &BasicSigOf<T>>,
        registry_version: RegistryVersion,
    ) -> CryptoResult<BasicSignatureBatch<T>> {
        self.crypto_component
            .combine_basic_sig(signatures, registry_version)
    }

    fn verify_basic_sig_batch(
        &self,
        signature: &BasicSignatureBatch<T>,
        message: &T,
        registry_version: RegistryVersion,
    ) -> CryptoResult<()> {
        self.crypto_component
            .verify_basic_sig_batch(signature, message, registry_version)
    }
}

impl<C: CryptoServiceProvider, T: Signable> MultiSigVerifier<T> for TempCryptoComponentGeneric<C> {
    fn verify_multi_sig_individual(
        &self,
        signature: &IndividualMultiSigOf<T>,
        message: &T,
        signer: NodeId,
        registry_version: RegistryVersion,
    ) -> CryptoResult<()> {
        self.crypto_component.verify_multi_sig_individual(
            signature,
            message,
            signer,
            registry_version,
        )
    }

    fn combine_multi_sig_individuals(
        &self,
        signatures: BTreeMap<NodeId, IndividualMultiSigOf<T>>,
        registry_version: RegistryVersion,
    ) -> CryptoResult<CombinedMultiSigOf<T>> {
        self.crypto_component
            .combine_multi_sig_individuals(signatures, registry_version)
    }

    fn verify_multi_sig_combined(
        &self,
        signature: &CombinedMultiSigOf<T>,
        message: &T,
        signers: BTreeSet<NodeId>,
        registry_version: RegistryVersion,
    ) -> CryptoResult<()> {
        self.crypto_component.verify_multi_sig_combined(
            signature,
            message,
            signers,
            registry_version,
        )
    }
}

impl<C: CryptoServiceProvider, T: Signable> ThresholdSigVerifier<T>
    for TempCryptoComponentGeneric<C>
{
    fn verify_threshold_sig_share(
        &self,
        signature: &ThresholdSigShareOf<T>,
        message: &T,
        dkg_id: DkgId,
        signer: NodeId,
    ) -> CryptoResult<()> {
        self.crypto_component
            .verify_threshold_sig_share(signature, message, dkg_id, signer)
    }

    fn combine_threshold_sig_shares(
        &self,
        shares: BTreeMap<NodeId, ThresholdSigShareOf<T>>,
        dkg_id: DkgId,
    ) -> CryptoResult<CombinedThresholdSigOf<T>> {
        self.crypto_component
            .combine_threshold_sig_shares(shares, dkg_id)
    }

    fn verify_threshold_sig_combined(
        &self,
        signature: &CombinedThresholdSigOf<T>,
        message: &T,
        dkg_id: DkgId,
    ) -> CryptoResult<()> {
        self.crypto_component
            .verify_threshold_sig_combined(signature, message, dkg_id)
    }
}

impl<C: CryptoServiceProvider, T: Signable> ThresholdSigVerifierByPublicKey<T>
    for TempCryptoComponentGeneric<C>
{
    fn verify_combined_threshold_sig_by_public_key(
        &self,
        signature: &CombinedThresholdSigOf<T>,
        message: &T,
        subnet_id: SubnetId,
        registry_version: RegistryVersion,
    ) -> CryptoResult<()> {
        self.crypto_component
            .verify_combined_threshold_sig_by_public_key(
                signature,
                message,
                subnet_id,
                registry_version,
            )
    }
}
