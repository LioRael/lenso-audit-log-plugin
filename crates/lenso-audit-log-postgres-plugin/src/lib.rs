//! PostgreSQL-backed, append-only Audit Log behavior for Lenso applications.

mod model;
mod operator;
mod repository;
mod schema;
mod storage;

#[cfg(test)]
mod tests;

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fmt,
    rc::Rc,
    time::Duration,
};

use lenso::prelude::*;
use lenso_capability_audit_log as audit;
use lenso_capability_audit_log::{
    AppendEventError, AppendEventRequest, AppendEventResponse, GetEventError, GetEventRequest,
    GetEventResponse, ListEventsError, ListEventsRequest, ListEventsResponse,
};
use lenso_capability_secrets as secrets;
use lenso_capability_secrets::{ResolveRequest, SecretsClient, SecretsInvocationError};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::{schema::schema_plan, storage::AuditStore};

pub use operator::{
    AuditLogOperator, AuditLogOperatorError, LegacyAdoptionOutcome, LegacyAdoptionRefusal,
};
pub use schema::AUDIT_LOG_SCHEMA;

const DEPENDENCY_TIMEOUT: Duration = Duration::from_secs(10);

/// Immutable policy and secret reference for one Audit Log Plugin Instance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuditLogConfig {
    database_url_secret: String,
    writer_instances: Vec<String>,
    reader_instances: Vec<String>,
    #[serde(default)]
    reader_scopes: BTreeMap<String, Vec<AuditReadScope>>,
}

pub use lenso_audit_log_core::AuditReadScope;

impl AuditLogConfig {
    /// Creates validated Audit Log policy for exact writer and reader Instances.
    pub fn new(
        database_url_secret: impl Into<String>,
        writer_instances: Vec<String>,
        reader_instances: Vec<String>,
    ) -> Result<Self, AuditLogConfigError> {
        let config = Self {
            database_url_secret: database_url_secret.into(),
            writer_instances,
            reader_instances,
            reader_scopes: BTreeMap::new(),
        };
        config.validate()?;
        Ok(config)
    }

    /// Restricts one allowed reader to exact scopes before database reads.
    pub fn with_reader_scopes(
        mut self,
        reader: impl Into<String>,
        scopes: Vec<AuditReadScope>,
    ) -> Result<Self, AuditLogConfigError> {
        self.reader_scopes.insert(reader.into(), scopes);
        self.validate()?;
        Ok(self)
    }

    #[cfg(test)]
    fn reader_admits_scope(&self, reader: &str, kind: Option<&str>, id: Option<&str>) -> bool {
        lenso_audit_log_core::AuditPolicy {
            writer_instances: self.writer_instances.clone(),
            reader_instances: self.reader_instances.clone(),
            reader_scopes: self.reader_scopes.clone(),
        }
        .reader_admits_scope(reader, kind, id)
    }

    fn validate(&self) -> Result<(), AuditLogConfigError> {
        if !valid_secret_reference(&self.database_url_secret) {
            return Err(AuditLogConfigError::InvalidSecretReference);
        }
        validate_callers(&self.writer_instances, CallerRole::Writer)?;
        validate_callers(&self.reader_instances, CallerRole::Reader)?;
        if self.reader_scopes.iter().any(|(reader, scopes)| {
            !self.reader_instances.contains(reader)
                || scopes.is_empty()
                || scopes.len() > 64
                || scopes
                    .iter()
                    .any(|scope| !valid_name(&scope.kind, 128) || !valid_name(&scope.id, 512))
        }) {
            return Err(AuditLogConfigError::InvalidReadScope);
        }
        schema_plan().map_err(|_| AuditLogConfigError::InvalidSchemaPlan)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallerRole {
    Writer,
    Reader,
}

impl fmt::Display for CallerRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Writer => "writer",
            Self::Reader => "reader",
        })
    }
}

/// Invalid immutable Audit Log configuration supplied by App Composition.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum AuditLogConfigError {
    #[error("invalid database URL secret reference")]
    InvalidSecretReference,
    #[error("at least one authorized {role} Instance is required")]
    EmptyCallers { role: CallerRole },
    #[error("invalid authorized {role} Instance")]
    InvalidCaller { role: CallerRole },
    #[error("authorized {role} Instances must not contain duplicates")]
    DuplicateCaller { role: CallerRole },
    #[error("invalid reader scope ceiling")]
    InvalidReadScope,
    #[error("the fixed Audit Log schema plan is invalid")]
    InvalidSchemaPlan,
}

fn validate_config(config: &AuditLogConfig) -> Result<(), RuntimeFailure> {
    config
        .validate()
        .map_err(|error| RuntimeFailure::InvalidResolvedPlan {
            detail: format!("Audit Log configuration is invalid: {error}"),
        })
}

#[lenso::plugin(
    lifecycle,
    configuration_schema = "configuration.schema.json",
    validate = validate_config
)]
#[derive(Clone)]
struct PostgresAuditLogPlugin {
    #[config]
    config: AuditLogConfig,
    secrets: Port<secrets::SecretsClient>,
    state: Rc<RefCell<Option<PreparedAuditLog>>>,
}

#[derive(Clone, Debug)]
struct PreparedAuditLog {
    store: AuditStore,
}

impl fmt::Debug for PostgresAuditLogPlugin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PostgresAuditLogPlugin")
            .field("secrets", &self.secrets)
            .field("prepared", &self.state.borrow().is_some())
            .field("writer_count", &self.config.writer_instances.len())
            .field("reader_count", &self.config.reader_instances.len())
            .finish()
    }
}

#[lenso::provides(audit::AuditLog)]
impl PostgresAuditLogPlugin {
    async fn append_event(
        &self,
        context: Ctx,
        request: AppendEventRequest,
    ) -> PluginResult<AppendEventResponse, AppendEventError> {
        self.service().append_event(context, request).await
    }
    async fn get_event(
        &self,
        context: Ctx,
        request: GetEventRequest,
    ) -> PluginResult<GetEventResponse, GetEventError> {
        self.service().get_event(context, request).await
    }
    async fn list_events(
        &self,
        context: Ctx,
        request: ListEventsRequest,
    ) -> PluginResult<ListEventsResponse, ListEventsError> {
        self.service().list_events(context, request).await
    }
}

impl PostgresAuditLogPlugin {
    fn service(&self) -> lenso_audit_log_core::AuditService<AuditStore> {
        lenso_audit_log_core::AuditService {
            config: lenso_audit_log_core::AuditPolicy {
                writer_instances: self.config.writer_instances.clone(),
                reader_instances: self.config.reader_instances.clone(),
                reader_scopes: self.config.reader_scopes.clone(),
            },
            store: self
                .state
                .borrow()
                .as_ref()
                .map_or(AuditStore::Unprepared, |prepared| prepared.store.clone()),
        }
    }
}

impl Lifecycle for PostgresAuditLogPlugin {
    async fn prepare(&self, context: PrepareContext) -> Result<(), RuntimeFailure> {
        let dependencies = context.dependencies().clone();
        let secrets = SecretsClient::from_dependencies(&dependencies)?;
        let invocation =
            dependencies.invocation_context_after(DEPENDENCY_TIMEOUT, context.cancellation())?;
        let database_url = secrets
            .resolve_with_context(
                invocation,
                ResolveRequest {
                    reference: self.config.database_url_secret.clone(),
                },
            )
            .await
            .map_err(|error| match error {
                SecretsInvocationError::Domain(_) => RuntimeFailure::PluginFailure {
                    detail: format!(
                        "Audit Log database URL secret `{}` was rejected",
                        self.config.database_url_secret
                    ),
                },
                SecretsInvocationError::Runtime(error) => error,
            })?;
        let database_url = Zeroizing::new(database_url.value);
        let postgres = operator::prepare_managed(&database_url)
            .await
            .map_err(|error| RuntimeFailure::PluginFailure {
                detail: format!("Audit Log storage is unavailable: {error}"),
            })?;
        if self
            .state
            .replace(Some(PreparedAuditLog {
                store: AuditStore::Postgres(postgres),
            }))
            .is_some()
        {
            return Err(RuntimeFailure::Internal {
                detail: "Audit Log generation was prepared more than once".to_owned(),
            });
        }
        Ok(())
    }

    async fn deactivate(&self, _context: DeactivateContext) -> Result<(), RuntimeFailure> {
        let prepared = self.state.borrow_mut().take();
        if let Some(prepared) = prepared {
            prepared.store.close().await;
        }
        Ok(())
    }
}

fn validate_callers(values: &[String], role: CallerRole) -> Result<(), AuditLogConfigError> {
    if values.is_empty() {
        return Err(AuditLogConfigError::EmptyCallers { role });
    }
    if values.len() > 1_024 || values.iter().any(|value| !valid_name(value, 256)) {
        return Err(AuditLogConfigError::InvalidCaller { role });
    }
    let unique = values.iter().collect::<BTreeSet<_>>();
    if unique.len() != values.len() {
        return Err(AuditLogConfigError::DuplicateCaller { role });
    }
    Ok(())
}

fn valid_name(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':' | b'/')
        })
}

fn valid_secret_reference(reference: &str) -> bool {
    !reference.is_empty()
        && reference.len() <= 256
        && !reference.starts_with('/')
        && !reference.ends_with('/')
        && !reference.contains("//")
        && reference
            .split('/')
            .all(|segment| segment != "." && segment != "..")
        && reference
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
}
