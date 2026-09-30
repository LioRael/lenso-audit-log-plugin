//! Owner-private validation and service shared by `PostgreSQL` and D1 implementations.
pub mod model;
use lenso::prelude::*;
use lenso_capability_audit_log::{
    AppendEventError, AppendEventRequest, AppendEventResponse, AppendEventResponseEvent,
    GetEventError, GetEventRequest, GetEventResponse, GetEventResponseEvent, ListEventsError,
    ListEventsRequest, ListEventsResponse, ListEventsResponseEventsItem,
    ListEventsResponseNextCursor,
};
use model::{EventFilter, NewAuditEvent, ProjectionError, StoredEvent, validate_event_id};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
/// An exact opaque scope ceiling for a trusted read adapter.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuditReadScope {
    pub kind: String,
    pub id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuditPolicy {
    pub writer_instances: Vec<String>,
    pub reader_instances: Vec<String>,
    #[serde(default)]
    pub reader_scopes: BTreeMap<String, Vec<AuditReadScope>>,
}
impl AuditPolicy {
    pub fn validate(&self) -> Result<(), RuntimeFailure> {
        for callers in [&self.writer_instances, &self.reader_instances] {
            if callers.is_empty()
                || callers.len() > 1024
                || callers.iter().any(|s| !valid_name(s, 256))
                || callers.iter().collect::<BTreeSet<_>>().len() != callers.len()
            {
                return Err(invalid_policy());
            }
        }
        if self.reader_scopes.iter().any(|(reader, scopes)| {
            !self.reader_instances.contains(reader)
                || scopes.is_empty()
                || scopes.len() > 64
                || scopes
                    .iter()
                    .any(|s| !valid_name(&s.kind, 128) || !valid_name(&s.id, 512))
        }) {
            return Err(invalid_policy());
        }
        Ok(())
    }
    pub fn reader_admits_scope(&self, reader: &str, kind: Option<&str>, id: Option<&str>) -> bool {
        self.reader_scopes.get(reader).is_none_or(|scopes| {
            scopes
                .iter()
                .any(|scope| Some(scope.kind.as_str()) == kind && Some(scope.id.as_str()) == id)
        })
    }
}
fn invalid_policy() -> RuntimeFailure {
    RuntimeFailure::InvalidResolvedPlan {
        detail: "invalid Audit caller/scope policy".into(),
    }
}
fn valid_name(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':' | b'/')
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreError {
    IdempotencyConflict,
    Unavailable,
}
#[allow(async_fn_in_trait)]
pub trait EventStore: Clone {
    fn fresh_id(&self) -> Result<String, PluginError<AppendEventError>>;
    async fn append_event(&self, event: NewAuditEvent) -> Result<StoredEvent, StoreError>;
    async fn list_events(&self, filter: &EventFilter) -> Result<Vec<StoredEvent>, StoreError>;
    async fn get_event(&self, id: &str) -> Result<Option<StoredEvent>, StoreError>;
}
#[derive(Clone, Debug)]
pub struct AuditService<S> {
    pub config: AuditPolicy,
    pub store: S,
}
impl<S: EventStore> AuditService<S> {
    pub async fn append_event(
        &self,
        context: Ctx,
        request: AppendEventRequest,
    ) -> PluginResult<AppendEventResponse, AppendEventError> {
        let Some(source_instance) = authorized_caller(&context, &self.config.writer_instances)
        else {
            return Err(PluginError::domain(AppendEventError::Unauthorized));
        };
        let needs_id = request.idempotency_key.is_none();
        let mut event = NewAuditEvent::from_request_with_id(request, source_instance, || {
            "audit_evt_pending".into()
        })
        .map_err(PluginError::domain)?;
        if needs_id {
            event.id = self.store.fresh_id()?;
        }
        let stored = self
            .store
            .append_event(event)
            .await
            .map_err(|error| match error {
                StoreError::IdempotencyConflict => {
                    PluginError::domain(AppendEventError::IdempotencyConflict)
                }
                StoreError::Unavailable => PluginError::runtime(unavailable()),
            })?;
        let event = stored
            .project::<AppendEventResponseEvent>()
            .map_err(|error| PluginError::runtime(projection_failure(&error)))?;
        Ok(AppendEventResponse { event })
    }

    pub async fn get_event(
        &self,
        context: Ctx,
        request: GetEventRequest,
    ) -> PluginResult<GetEventResponse, GetEventError> {
        let reader = authorized_caller(&context, &self.config.reader_instances)
            .ok_or_else(|| PluginError::domain(GetEventError::Unauthorized))?;
        validate_event_id(&request.id).map_err(PluginError::domain)?;
        let stored = self
            .store
            .get_event(&request.id)
            .await
            .map_err(|error| PluginError::runtime(store_failure(error)))?
            .ok_or_else(|| PluginError::domain(GetEventError::NotFound))?;
        if !self.config.reader_admits_scope(
            reader,
            stored.scope_type.as_deref(),
            stored.scope_id.as_deref(),
        ) {
            return Err(PluginError::domain(GetEventError::NotFound));
        }
        let event = stored
            .project::<GetEventResponseEvent>()
            .map_err(|error| PluginError::runtime(projection_failure(&error)))?;
        Ok(GetEventResponse { event })
    }

    pub async fn list_events(
        &self,
        context: Ctx,
        request: ListEventsRequest,
    ) -> PluginResult<ListEventsResponse, ListEventsError> {
        let reader = authorized_caller(&context, &self.config.reader_instances)
            .ok_or_else(|| PluginError::domain(ListEventsError::Unauthorized))?;
        if !self.config.reader_admits_scope(
            reader,
            request.scope_type.as_deref(),
            request.scope_id.as_deref(),
        ) {
            return Err(PluginError::domain(ListEventsError::Unauthorized));
        }
        let filter = EventFilter::from_request(request).map_err(PluginError::domain)?;
        let mut stored = self
            .store
            .list_events(&filter)
            .await
            .map_err(|error| PluginError::runtime(store_failure(error)))?;
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
fn authorized_caller<'a>(context: &'a Ctx, allowed: &[String]) -> Option<&'a str> {
    context
        .caller_instance()
        .filter(|s| allowed.iter().any(|a| a == s))
}
fn unavailable() -> RuntimeFailure {
    RuntimeFailure::PluginFailure {
        detail: "Audit storage unavailable; a dispatched append may have committed".into(),
    }
}
fn store_failure(_: StoreError) -> RuntimeFailure {
    unavailable()
}
fn projection_failure(error: &ProjectionError) -> RuntimeFailure {
    RuntimeFailure::Internal {
        detail: format!("Audit Log generated projection failed: {error}"),
    }
}
