use ethabi::Contract;
use failure;
use failure::{Error, SyncFailure};
use futures::stream;
use parity_wasm;
use parity_wasm::elements::Module;
use serde::de;
use serde::ser;
use serde_yaml;
use slog::{info, Logger};
use std::fmt;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;
use tokio::prelude::*;
use web3::types::{Address, H256};

use crate::components::link_resolver::LinkResolver;
use crate::components::store::StoreError;
use crate::data::query::QueryExecutionError;
use crate::data::schema::Schema;
use crate::data::subgraph::schema::{
    EthereumBlockHandlerEntity, EthereumCallHandlerEntity, EthereumContractAbiEntity,
    EthereumContractDataSourceEntity, EthereumContractDataSourceTemplateEntity,
    EthereumContractDataSourceTemplateSourceEntity, EthereumContractEventHandlerEntity,
    EthereumContractMappingEntity, EthereumContractSourceEntity, SUBGRAPHS_ID,
};
use crate::prelude::{format_err, Deserialize, Fail, Serialize};
use crate::util::ethereum::string_to_h256;

/// Rust representation of the GraphQL schema for a `SubgraphManifest`.
pub mod schema;

/// Deserialize an Address (with or without '0x' prefix).
fn deserialize_address<'de, D>(deserializer: D) -> Result<Option<Address>, D::Error>
where
    D: de::Deserializer<'de>,
{
    use serde::de::Error;

    let s: String = de::Deserialize::deserialize(deserializer)?;
    let address = s.trim_start_matches("0x");
    Address::from_str(address)
        .map_err(D::Error::custom)
        .map(|addr| Some(addr))
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SubgraphDeploymentId(String);

impl SubgraphDeploymentId {
    pub fn new(s: impl Into<String>) -> Result<Self, ()> {
        let s = s.into();

        // Enforce length limit
        if s.len() > 46 {
            return Err(());
        }

        // Check that the ID contains only allowed characters.
        if !s.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(());
        }

        Ok(SubgraphDeploymentId(s))
    }

    pub fn to_ipfs_link(&self) -> Link {
        Link {
            link: format!("/ipfs/{}", self),
        }
    }

    /// Return true if this is the id of the special
    /// "subgraph of subgraphs" that contains metadata about everything
    pub fn is_meta(&self) -> bool {
        self.0 == *SUBGRAPHS_ID.0
    }
}

impl Deref for SubgraphDeploymentId {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for SubgraphDeploymentId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl ser::Serialize for SubgraphDeploymentId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> de::Deserialize<'de> for SubgraphDeploymentId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        let s: String = de::Deserialize::deserialize(deserializer)?;
        SubgraphDeploymentId::new(s.clone())
            .map_err(|()| de::Error::invalid_value(de::Unexpected::Str(&s), &"valid subgraph name"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SubgraphName(String);

impl SubgraphName {
    pub fn new(s: impl Into<String>) -> Result<Self, ()> {
        let s = s.into();

        // Note: these validation rules must be kept consistent with the validation rules
        // implemented in any other components that rely on subgraph names.

        // Enforce length limits
        if s.is_empty() || s.len() > 255 {
            return Err(());
        }

        // Check that the name contains only allowed characters.
        if !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '/')
        {
            return Err(());
        }

        // Parse into components and validate each
        for part in s.split("/") {
            // Each part must be non-empty and not too long
            if part.is_empty() || part.len() > 32 {
                return Err(());
            }

            // To keep URLs unambiguous, reserve the token "graphql"
            if part == "graphql" {
                return Err(());
            }

            // Part should not start or end with a special character.
            let first_char = part.chars().next().unwrap();
            let last_char = part.chars().last().unwrap();
            if !first_char.is_ascii_alphanumeric()
                || !last_char.is_ascii_alphanumeric()
                || !part.chars().any(|c| c.is_ascii_alphabetic())
            {
                return Err(());
            }
        }

        Ok(SubgraphName(s))
    }
}

impl fmt::Display for SubgraphName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl ser::Serialize for SubgraphName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> de::Deserialize<'de> for SubgraphName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        let s: String = de::Deserialize::deserialize(deserializer)?;
        SubgraphName::new(s.clone())
            .map_err(|()| de::Error::invalid_value(de::Unexpected::Str(&s), &"valid subgraph name"))
    }
}

#[test]
fn test_subgraph_name_validation() {
    assert!(SubgraphName::new("a").is_ok());
    assert!(SubgraphName::new("a/a").is_ok());
    assert!(SubgraphName::new("a-lOng-name_with_0ne-component").is_ok());
    assert!(SubgraphName::new("a-long-name_with_one-3omponent").is_ok());
    assert!(SubgraphName::new("a/b_c").is_ok());
    assert!(SubgraphName::new("A/Z-Z").is_ok());
    assert!(SubgraphName::new("a1/A-A").is_ok());
    assert!(SubgraphName::new("aaa/a1").is_ok());
    assert!(SubgraphName::new("1a/aaaa").is_ok());
    assert!(SubgraphName::new("aaaa/1a").is_ok());
    assert!(SubgraphName::new("2nena4test/lala").is_ok());

    assert!(SubgraphName::new("").is_err());
    assert!(SubgraphName::new("/a").is_err());
    assert!(SubgraphName::new("a/").is_err());
    assert!(SubgraphName::new("a//a").is_err());
    assert!(SubgraphName::new("a/0").is_err());
    assert!(SubgraphName::new("a/_").is_err());
    assert!(SubgraphName::new("a/a_").is_err());
    assert!(SubgraphName::new("a/_a").is_err());
    assert!(SubgraphName::new("aaaa aaaaa").is_err());
    assert!(SubgraphName::new("aaaa!aaaaa").is_err());
    assert!(SubgraphName::new("aaaa+aaaaa").is_err());
    assert!(SubgraphName::new("a/graphql").is_err());
    assert!(SubgraphName::new("graphql/a").is_err());
    assert!(SubgraphName::new("this-component-is-longer-than-the-length-limit").is_err());
}

/// Result of a creating a subgraph in the registar.
#[derive(Serialize)]
pub struct CreateSubgraphResult {
    /// The ID of the subgraph that was created.
    pub id: String,
}

#[derive(Fail, Debug)]
pub enum SubgraphRegistrarError {
    #[fail(display = "subgraph resolve error: {}", _0)]
    ResolveError(SubgraphManifestResolveError),
    #[fail(display = "subgraph already exists: {}", _0)]
    NameExists(String),
    #[fail(display = "subgraph name not found: {}", _0)]
    NameNotFound(String),
    #[fail(display = "Ethereum network not supported by registrar: {}", _0)]
    NetworkNotSupported(String),
    #[fail(display = "deployment not found: {}", _0)]
    DeploymentNotFound(String),
    #[fail(display = "deployment assignment unchanged: {}", _0)]
    DeploymentAssignmentUnchanged(String),
    #[fail(display = "subgraph registrar internal query error: {}", _0)]
    QueryExecutionError(QueryExecutionError),
    #[fail(display = "subgraph registrar error with store: {}", _0)]
    StoreError(StoreError),
    #[fail(display = "subgraph validation error: {:?}", _0)]
    ManifestValidationError(Vec<SubgraphManifestValidationError>),
    #[fail(display = "subgraph registrar error: {}", _0)]
    Unknown(failure::Error),
}

impl From<QueryExecutionError> for SubgraphRegistrarError {
    fn from(e: QueryExecutionError) -> Self {
        SubgraphRegistrarError::QueryExecutionError(e)
    }
}

impl From<StoreError> for SubgraphRegistrarError {
    fn from(e: StoreError) -> Self {
        SubgraphRegistrarError::StoreError(e)
    }
}

impl From<Error> for SubgraphRegistrarError {
    fn from(e: Error) -> Self {
        SubgraphRegistrarError::Unknown(e)
    }
}

impl From<SubgraphManifestValidationError> for SubgraphRegistrarError {
    fn from(e: SubgraphManifestValidationError) -> Self {
        SubgraphRegistrarError::ManifestValidationError(vec![e])
    }
}

#[derive(Fail, Debug)]
pub enum SubgraphAssignmentProviderError {
    #[fail(display = "Subgraph resolve error: {}", _0)]
    ResolveError(SubgraphManifestResolveError),
    #[fail(display = "Failed to load dynamic data sources: {}", _0)]
    DynamicDataSourcesError(failure::Error),
    /// Occurs when attempting to remove a subgraph that's not hosted.
    #[fail(display = "Subgraph with ID {} already running", _0)]
    AlreadyRunning(SubgraphDeploymentId),
    #[fail(display = "Subgraph with ID {} is not running", _0)]
    NotRunning(SubgraphDeploymentId),
    /// Occurs when a subgraph's GraphQL schema is invalid.
    #[fail(display = "GraphQL schema error: {}", _0)]
    SchemaValidationError(failure::Error),
    #[fail(
        display = "Error building index for subgraph {}, entity {} and attribute {}",
        _0, _1, _2
    )]
    BuildIndexesError(String, String, String),
    #[fail(display = "Subgraph provider error: {}", _0)]
    Unknown(failure::Error),
}

impl From<Error> for SubgraphAssignmentProviderError {
    fn from(e: Error) -> Self {
        SubgraphAssignmentProviderError::Unknown(e)
    }
}

impl From<::diesel::result::Error> for SubgraphAssignmentProviderError {
    fn from(e: ::diesel::result::Error) -> Self {
        SubgraphAssignmentProviderError::Unknown(e.into())
    }
}

/// Events emitted by [SubgraphAssignmentProvider](trait.SubgraphAssignmentProvider.html) implementations.
#[derive(Debug, PartialEq)]
pub enum SubgraphAssignmentProviderEvent {
    /// A subgraph with the given manifest should start processing.
    SubgraphStart(SubgraphManifest),
    /// The subgraph with the given ID should stop processing.
    SubgraphStop(SubgraphDeploymentId),
}

#[derive(Fail, Debug)]
pub enum SubgraphManifestValidationError {
    #[fail(display = "subgraph source address is required")]
    SourceAddressRequired,
    #[fail(display = "subgraph cannot index data from different Ethereum networks")]
    MultipleEthereumNetworks,
    #[fail(display = "subgraph must have at least one Ethereum network data source")]
    EthereumNetworkRequired,
    #[fail(display = "subgraph data source has too many similar block handlers")]
    DataSourceBlockHandlerLimitExceeded,
}

#[derive(Fail, Debug)]
pub enum SubgraphManifestResolveError {
    #[fail(display = "parse error: {}", _0)]
    ParseError(serde_yaml::Error),
    #[fail(display = "subgraph is not UTF-8")]
    NonUtf8,
    #[fail(display = "subgraph is not valid YAML")]
    InvalidFormat,
    #[fail(display = "resolve error: {}", _0)]
    ResolveError(failure::Error),
}

impl From<serde_yaml::Error> for SubgraphManifestResolveError {
    fn from(e: serde_yaml::Error) -> Self {
        SubgraphManifestResolveError::ParseError(e)
    }
}

/// IPLD link.
#[derive(Clone, Debug, Default, Hash, Eq, PartialEq, Deserialize)]
pub struct Link {
    #[serde(rename = "/")]
    pub link: String,
}

impl From<String> for Link {
    fn from(s: String) -> Self {
        Self { link: s }
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Deserialize)]
pub struct SchemaData {
    pub file: Link,
}

impl SchemaData {
    pub fn resolve(
        self,
        id: SubgraphDeploymentId,
        resolver: &impl LinkResolver,
        logger: Logger,
    ) -> impl Future<Item = Schema, Error = failure::Error> + Send {
        info!(logger, "Resolve schema"; "link" => &self.file.link);

        resolver
            .cat(&logger, &self.file)
            .and_then(|schema_bytes| Schema::parse(&String::from_utf8(schema_bytes)?, id))
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Deserialize)]
pub struct Source {
    #[serde(default, deserialize_with = "deserialize_address")]
    pub address: Option<Address>,
    pub abi: String,
}

impl From<EthereumContractSourceEntity> for Source {
    fn from(entity: EthereumContractSourceEntity) -> Self {
        Self {
            address: entity.address,
            abi: entity.abi,
        }
    }
}

#[derive(Clone, Debug, Default, Hash, Eq, PartialEq, Deserialize)]
pub struct TemplateSource {
    pub abi: String,
}

impl From<EthereumContractDataSourceTemplateSourceEntity> for TemplateSource {
    fn from(entity: EthereumContractDataSourceTemplateSourceEntity) -> Self {
        Self { abi: entity.abi }
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Deserialize)]
pub struct UnresolvedMappingABI {
    pub name: String,
    pub file: Link,
}

impl From<EthereumContractAbiEntity> for UnresolvedMappingABI {
    fn from(entity: EthereumContractAbiEntity) -> Self {
        Self {
            name: entity.name,
            file: entity.file.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct MappingABI {
    pub name: String,
    pub contract: Contract,
    pub link: Link,
}

impl UnresolvedMappingABI {
    pub fn resolve(
        self,
        resolver: &impl LinkResolver,
        logger: Logger,
    ) -> impl Future<Item = MappingABI, Error = failure::Error> + Send {
        info!(
            logger,
            "Resolve ABI";
            "name" => &self.name,
            "link" => &self.file.link
        );

        resolver
            .cat(&logger, &self.file)
            .and_then(|contract_bytes| {
                let contract = Contract::load(&*contract_bytes).map_err(SyncFailure::new)?;
                Ok(MappingABI {
                    name: self.name,
                    contract,
                    link: self.file,
                })
            })
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Deserialize)]
pub struct MappingBlockHandler {
    pub handler: String,
    pub filter: Option<BlockHandlerFilter>,
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum BlockHandlerFilter {
    // Call filter will trigger on all blocks where the data source contract
    // address has been called
    Call,
}

impl From<EthereumBlockHandlerEntity> for MappingBlockHandler {
    fn from(entity: EthereumBlockHandlerEntity) -> Self {
        Self {
            handler: entity.handler,
            filter: None,
        }
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Deserialize)]
pub struct MappingCallHandler {
    pub function: String,
    pub handler: String,
}

impl From<EthereumCallHandlerEntity> for MappingCallHandler {
    fn from(entity: EthereumCallHandlerEntity) -> Self {
        Self {
            function: entity.function,
            handler: entity.handler,
        }
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Deserialize)]
pub struct MappingEventHandler {
    pub event: String,
    pub topic0: Option<H256>,
    pub handler: String,
}

impl MappingEventHandler {
    pub fn topic0(&self) -> H256 {
        self.topic0
            .unwrap_or_else(|| string_to_h256(&self.event.replace("indexed ", "")))
    }
}

impl From<EthereumContractEventHandlerEntity> for MappingEventHandler {
    fn from(entity: EthereumContractEventHandlerEntity) -> Self {
        Self {
            event: entity.event,
            topic0: entity.topic0,
            handler: entity.handler,
        }
    }
}

#[derive(Clone, Debug, Default, Hash, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedMapping {
    pub kind: String,
    pub api_version: String,
    pub language: String,
    pub entities: Vec<String>,
    pub abis: Vec<UnresolvedMappingABI>,
    #[serde(default)]
    pub block_handlers: Vec<MappingBlockHandler>,
    #[serde(default)]
    pub call_handlers: Vec<MappingCallHandler>,
    #[serde(default)]
    pub event_handlers: Vec<MappingEventHandler>,
    pub file: Link,
}

#[derive(Clone, Debug)]
pub struct Mapping {
    pub kind: String,
    pub api_version: String,
    pub language: String,
    pub entities: Vec<String>,
    pub abis: Vec<MappingABI>,
    pub block_handlers: Vec<MappingBlockHandler>,
    pub call_handlers: Vec<MappingCallHandler>,
    pub event_handlers: Vec<MappingEventHandler>,
    pub runtime: Arc<Module>,
    pub link: Link,
}

impl UnresolvedMapping {
    pub fn resolve(
        self,
        resolver: &impl LinkResolver,
        logger: Logger,
    ) -> impl Future<Item = Mapping, Error = failure::Error> + Send {
        let UnresolvedMapping {
            kind,
            api_version,
            language,
            entities,
            abis,
            block_handlers,
            call_handlers,
            event_handlers,
            file: link,
        } = self;

        info!(logger, "Resolve mapping"; "link" => &link.link);

        // resolve each abi
        stream::futures_ordered(
            abis.into_iter()
                .map(|unresolved_abi| unresolved_abi.resolve(resolver, logger.clone())),
        )
        .collect()
        .join(
            resolver.cat(&logger, &link).and_then(|module_bytes| {
                Ok(Arc::new(parity_wasm::deserialize_buffer(&module_bytes)?))
            }),
        )
        .map(move |(abis, runtime)| Mapping {
            kind,
            api_version,
            language,
            entities,
            abis,
            block_handlers: block_handlers.clone(),
            call_handlers: call_handlers.clone(),
            event_handlers: event_handlers.clone(),
            runtime,
            link,
        })
    }
}

impl From<EthereumContractMappingEntity> for UnresolvedMapping {
    fn from(entity: EthereumContractMappingEntity) -> Self {
        Self {
            kind: entity.kind,
            api_version: entity.api_version,
            language: entity.language,
            entities: entity.entities,
            abis: entity.abis.into_iter().map(Into::into).collect(),
            event_handlers: entity.event_handlers.into_iter().map(Into::into).collect(),
            call_handlers: entity.call_handlers.into_iter().map(Into::into).collect(),
            block_handlers: entity.block_handlers.into_iter().map(Into::into).collect(),
            file: entity.file.into(),
        }
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Deserialize)]
pub struct BaseDataSource<M, T> {
    pub kind: String,
    pub network: Option<String>,
    pub name: String,
    pub source: Source,
    pub mapping: M,
    #[serde(default)]
    pub templates: Vec<T>, // Deprecated in manifest spec version 0.0.2
}

pub type UnresolvedDataSource = BaseDataSource<UnresolvedMapping, UnresolvedDataSourceTemplate>;
pub type DataSource = BaseDataSource<Mapping, DataSourceTemplate>;

impl UnresolvedDataSource {
    pub fn resolve(
        self,
        resolver: &impl LinkResolver,
        logger: Logger,
    ) -> impl Future<Item = DataSource, Error = failure::Error> {
        let UnresolvedDataSource {
            kind,
            network,
            name,
            source,
            mapping,
            templates,
        } = self;

        info!(logger, "Resolve data source"; "name" => &name);

        mapping
            .resolve(resolver, logger.clone())
            .join(
                stream::futures_ordered(
                    templates
                        .into_iter()
                        .map(|template| template.resolve(resolver, logger.clone())),
                )
                .collect(),
            )
            .map(|(mapping, templates)| DataSource {
                kind,
                network,
                name,
                source,
                mapping,
                templates,
            })
    }
}

impl DataSource {
    pub fn try_from_template(
        template: DataSourceTemplate,
        params: &Vec<String>,
    ) -> Result<Self, failure::Error> {
        // Obtain the address from the parameters
        let string = params
            .get(0)
            .ok_or_else(|| {
                format_err!(
                    "Failed to create data source from template `{}`: address parameter is missing",
                    template.name
                )
            })?
            .trim_start_matches("0x");

        let address = Address::from_str(string).map_err(|e| {
            format_err!(
                "Failed to create data source from template `{}`: invalid address provided: {}",
                template.name,
                e
            )
        })?;

        Ok(DataSource {
            kind: template.kind,
            network: template.network,
            name: template.name,
            source: Source {
                address: Some(address),
                abi: template.source.abi,
            },
            mapping: template.mapping,
            templates: Vec::new(),
        })
    }
}

impl From<EthereumContractDataSourceEntity> for UnresolvedDataSource {
    fn from(entity: EthereumContractDataSourceEntity) -> Self {
        Self {
            kind: entity.kind,
            network: entity.network,
            name: entity.name,
            source: entity.source.into(),
            mapping: entity.mapping.into(),
            templates: entity.templates.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, Default, Hash, Eq, PartialEq, Deserialize)]
pub struct BaseDataSourceTemplate<M> {
    pub kind: String,
    pub network: Option<String>,
    pub name: String,
    pub source: TemplateSource,
    pub mapping: M,
}

impl From<EthereumContractDataSourceTemplateEntity> for UnresolvedDataSourceTemplate {
    fn from(entity: EthereumContractDataSourceTemplateEntity) -> Self {
        Self {
            kind: entity.kind,
            network: entity.network,
            name: entity.name,
            source: entity.source.into(),
            mapping: entity.mapping.into(),
        }
    }
}

pub type UnresolvedDataSourceTemplate = BaseDataSourceTemplate<UnresolvedMapping>;
pub type DataSourceTemplate = BaseDataSourceTemplate<Mapping>;

impl UnresolvedDataSourceTemplate {
    pub fn resolve(
        self,
        resolver: &impl LinkResolver,
        logger: Logger,
    ) -> impl Future<Item = DataSourceTemplate, Error = failure::Error> {
        let UnresolvedDataSourceTemplate {
            kind,
            network,
            name,
            source,
            mapping,
        } = self;

        info!(logger, "Resolve data source template"; "name" => &name);

        mapping
            .resolve(resolver, logger)
            .map(|mapping| DataSourceTemplate {
                kind,
                network,
                name,
                source,
                mapping,
            })
    }
}

impl DataSourceTemplate {
    pub fn has_call_handler(&self) -> bool {
        !self.mapping.call_handlers.is_empty()
    }

    pub fn has_block_handler_with_call_filter(&self) -> bool {
        self.mapping
            .block_handlers
            .iter()
            .find(|handler| match handler.filter {
                Some(BlockHandlerFilter::Call) => true,
                _ => false,
            })
            .is_some()
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseSubgraphManifest<S, D, T> {
    pub id: SubgraphDeploymentId,
    pub location: String,
    pub spec_version: String,
    pub description: Option<String>,
    pub repository: Option<String>,
    pub schema: S,
    pub data_sources: Vec<D>,
    #[serde(default)]
    pub templates: Vec<T>,
}

/// Consider two subgraphs to be equal if they come from the same IPLD link.
impl<S, D, T> PartialEq for BaseSubgraphManifest<S, D, T> {
    fn eq(&self, other: &Self) -> bool {
        self.location == other.location
    }
}

pub type UnresolvedSubgraphManifest =
    BaseSubgraphManifest<SchemaData, UnresolvedDataSource, UnresolvedDataSourceTemplate>;
pub type SubgraphManifest = BaseSubgraphManifest<Schema, DataSource, DataSourceTemplate>;

impl SubgraphManifest {
    /// Entry point for resolving a subgraph definition.
    /// Right now the only supported links are of the form:
    /// `/ipfs/QmUmg7BZC1YP1ca66rRtWKxpXp77WgVHrnv263JtDuvs2k`
    pub fn resolve(
        link: Link,
        resolver: Arc<impl LinkResolver>,
        logger: Logger,
    ) -> impl Future<Item = Self, Error = SubgraphManifestResolveError> + Send {
        info!(logger, "Resolve manifest"; "link" => &link.link);

        resolver
            .cat(&logger, &link)
            .map_err(SubgraphManifestResolveError::ResolveError)
            .and_then(move |file_bytes| {
                let file = String::from_utf8(file_bytes.to_vec())
                    .map_err(|_| SubgraphManifestResolveError::NonUtf8)?;
                let mut raw: serde_yaml::Value = serde_yaml::from_str(&file)?;
                {
                    let raw_mapping = raw
                        .as_mapping_mut()
                        .ok_or(SubgraphManifestResolveError::InvalidFormat)?;

                    // Inject the IPFS hash as the ID of the subgraph
                    // into the definition.
                    raw_mapping.insert(
                        serde_yaml::Value::from("id"),
                        serde_yaml::Value::from(link.link.trim_start_matches("/ipfs/")),
                    );

                    // Inject the IPFS link as the location of the data
                    // source into the definition
                    raw_mapping.insert(
                        serde_yaml::Value::from("location"),
                        serde_yaml::Value::from(link.link),
                    );
                }
                // Parse the YAML data into an UnresolvedSubgraphManifest
                let unresolved: UnresolvedSubgraphManifest = serde_yaml::from_value(raw)?;
                Ok(unresolved)
            })
            .and_then(move |unresolved| {
                unresolved
                    .resolve(&*resolver, logger)
                    .map_err(SubgraphManifestResolveError::ResolveError)
            })
    }

    pub fn network_name(&self) -> Result<String, SubgraphManifestValidationError> {
        let mut ethereum_networks: Vec<Option<String>> = self
            .data_sources
            .iter()
            .cloned()
            .filter(|d| d.kind == "ethereum/contract".to_string())
            .map(|d| d.network)
            .collect();
        ethereum_networks.sort();
        ethereum_networks.dedup();
        match ethereum_networks.len() {
            0 => Err(SubgraphManifestValidationError::EthereumNetworkRequired),
            1 => match ethereum_networks.first().and_then(|n| n.clone()) {
                Some(n) => Ok(n),
                None => Err(SubgraphManifestValidationError::EthereumNetworkRequired),
            },
            _ => Err(SubgraphManifestValidationError::MultipleEthereumNetworks),
        }
    }
}

impl UnresolvedSubgraphManifest {
    pub fn resolve(
        self,
        resolver: &impl LinkResolver,
        logger: Logger,
    ) -> impl Future<Item = SubgraphManifest, Error = failure::Error> {
        let UnresolvedSubgraphManifest {
            id,
            location,
            spec_version,
            description,
            repository,
            schema,
            data_sources,
            templates,
        } = self;

        match semver::Version::parse(&spec_version) {
            // Before this check was introduced, there were already subgraphs in
            // the wild with spec version 0.0.3, due to confusion with the api
            // version. To avoid breaking those, we accept 0.0.3 though it
            // doesn't exist. In the future we should not use 0.0.3 as version
            // and skip to 0.0.4 to avoid ambiguity.
            Ok(ref ver) if *ver <= semver::Version::new(0, 0, 3) => {}
            _ => {
                return Box::new(future::err(format_err!(
                    "This Graph Node only supports manifest spec versions <= 0.0.2,
                    but subgraph `{}` uses `{}`",
                    id,
                    spec_version
                ))) as Box<dyn Future<Item = _, Error = _> + Send>;
            }
        }

        Box::new(
            schema
                .resolve(id.clone(), resolver, logger.clone())
                .join(
                    stream::futures_ordered(
                        data_sources
                            .into_iter()
                            .map(|ds| ds.resolve(resolver, logger.clone())),
                    )
                    .collect(),
                )
                .join(
                    stream::futures_ordered(
                        templates
                            .into_iter()
                            .map(|template| template.resolve(resolver, logger.clone())),
                    )
                    .collect(),
                )
                .map(|((schema, data_sources), templates)| SubgraphManifest {
                    id,
                    location,
                    spec_version,
                    description,
                    repository,
                    schema,
                    data_sources,
                    templates,
                }),
        )
    }
}
