//! PostgreSQL-backed, append-only Audit Log behavior for Lenso applications.

mod model;
mod operator;
mod repository;
mod schema;
mod storage;

#[cfg(test)]
mod tests;

use std::{cell::RefCell, collections::BTreeSet, fmt, rc::Rc, time::Duration};

use lenso::prelude::*;
use lenso_capability_audit_log as audit;
use lenso_capability_audit_log::{
    AppendEventError, AppendEventRequest, AppendEventResponse, AppendEventResponseEvent,
    GetEventError, GetEventRequest, GetEventResponse, GetEventResponseEvent, ListEventsError,
    ListEventsRequest, ListEventsResponse, ListEventsResponseEventsItem,
    ListEventsResponseNextCursor,
};
use lenso_capability_secrets as secrets;
use lenso_capability_secrets::{ResolveRequest, SecretsClient, SecretsInvocationError};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::{
    model::{EventFilter, NewAuditEvent, ProjectionError, StoredEvent, validate_event_id},
    schema::schema_plan,
    storage::{AuditStore, AuditStoreError},
};

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
}

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
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), AuditLogConfigError> {
        if !valid_secret_reference(&self.database_url_secret) {
            return Err(AuditLogConfigError::InvalidSecretReference);
        }
        validate_callers(&self.writer_instances, CallerRole::Writer)?;
        validate_callers(&self.reader_instances, CallerRole::Reader)?;
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
        let Some(source_instance) =
            Self::authorized_caller(&context, &self.config.writer_instances)
        else {
            return Err(PluginError::domain(AppendEventError::Unauthorized));
        };
        let event =
            NewAuditEvent::from_request(request, source_instance).map_err(PluginError::domain)?;
        let stored = self
            .prepared()
            .map_err(PluginError::runtime)?
            .store
            .append_event(event)
            .await
            .map_err(|error| PluginError::runtime(storage_failure(&error)))?;
        let event = stored
            .project::<AppendEventResponseEvent>()
            .map_err(|error| PluginError::runtime(projection_failure(&error)))?;
        Ok(AppendEventResponse { event })
    }

    async fn get_event(
        &self,
        context: Ctx,
        request: GetEventRequest,
    ) -> PluginResult<GetEventResponse, GetEventError> {
        if !Self::authorized(&context, &self.config.reader_instances) {
            return Err(PluginError::domain(GetEventError::Unauthorized));
        }
        validate_event_id(&request.id).map_err(PluginError::domain)?;
        let stored = self
            .prepared()
            .map_err(PluginError::runtime)?
            .store
            .get_event(&request.id)
            .await
            .map_err(|error| PluginError::runtime(storage_failure(&error)))?
            .ok_or_else(|| PluginError::domain(GetEventError::NotFound))?;
        let event = stored
            .project::<GetEventResponseEvent>()
            .map_err(|error| PluginError::runtime(projection_failure(&error)))?;
        Ok(GetEventResponse { event })
    }

    async fn list_events(
        &self,
        context: Ctx,
        request: ListEventsRequest,
    ) -> PluginResult<ListEventsResponse, ListEventsError> {
        if !Self::authorized(&context, &self.config.reader_instances) {
            return Err(PluginError::domain(ListEventsError::Unauthorized));
        }
        let filter = EventFilter::from_request(request).map_err(PluginError::domain)?;
        let mut stored = self
            .prepared()
            .map_err(PluginError::runtime)?
            .store
            .list_events(&filter)
            .await
            .map_err(|error| PluginError::runtime(storage_failure(&error)))?;
        let limit = usize::try_from(filter.limit).expect("validated list limit fits usize");
        let has_next_page = stored.len() > limit;
        if has_next_page {
            stored.truncate(limit);
        }
        let next_cursor = if has_next_page {
            stored.last().map(|event| ListEventsResponseNextCursor {
                occurred_at: event.occurred_at.to_rfc3339(),
                id: event.id.clone(),
            })
        } else {
            None
        };
        let events = stored
            .iter()
            .map(StoredEvent::project::<ListEventsResponseEventsItem>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| PluginError::runtime(projection_failure(&error)))?;
        Ok(ListEventsResponse {
            events,
            next_cursor,
        })
    }
}

impl PostgresAuditLogPlugin {
    fn authorized(context: &Ctx, allowed: &[String]) -> bool {
        Self::authorized_caller(context, allowed).is_some()
    }

    fn authorized_caller<'a>(context: &'a Ctx, allowed: &[String]) -> Option<&'a str> {
        context
            .caller_instance()
            .filter(|caller| allowed.iter().any(|candidate| candidate == caller))
    }

    fn prepared(&self) -> Result<PreparedAuditLog, RuntimeFailure> {
        self.state
            .borrow()
            .clone()
            .ok_or(RuntimeFailure::PluginFailure {
                detail: "Audit Log Plugin is not prepared".to_owned(),
            })
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

fn storage_failure(error: &AuditStoreError) -> RuntimeFailure {
    RuntimeFailure::PluginFailure {
        detail: error.to_string(),
    }
}

fn projection_failure(error: &ProjectionError) -> RuntimeFailure {
    RuntimeFailure::Internal {
        detail: format!("Audit Log generated projection failed: {error}"),
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
