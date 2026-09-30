use std::{collections::BTreeMap, io};

use chrono::{DateTime, Datelike, Utc};
use lenso_capability_audit_log::{
    AppendEventError, AppendEventRequest, AppendEventRequestOutcome, AppendEventRequestSeverity,
    GetEventError, ListEventsError, ListEventsRequest, ListEventsRequestOutcome,
    ListEventsRequestSeverity,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
#[cfg(not(target_arch = "wasm32"))]
use uuid::Uuid;

pub const MAX_METADATA_BYTES: usize = 64 * 1024;
const MAX_METADATA_CONTAINER_ITEMS: usize = 1_024;
const MAX_METADATA_DEPTH: usize = 32;
const MAX_METADATA_KEY_CHARS: usize = 256;
const MAX_METADATA_NODES: usize = 16_384;
const MAX_METADATA_STRING_CHARS: usize = 65_536;
const MAX_TIMESTAMP_CHARS: usize = 64;

struct BoundedJsonCounter {
    encoded_bytes: usize,
    limit: usize,
}

impl BoundedJsonCounter {
    const fn new(limit: usize) -> Self {
        Self {
            encoded_bytes: 0,
            limit,
        }
    }
}

impl io::Write for BoundedJsonCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.encoded_bytes = self
            .encoded_bytes
            .checked_add(bytes.len())
            .filter(|encoded_bytes| *encoded_bytes <= self.limit)
            .ok_or_else(|| io::Error::other("audit metadata encoded size limit exceeded"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Success,
    Failure,
    Denied,
}

impl AuditOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Denied => "denied",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "success" => Some(Self::Success),
            "failure" => Some(Self::Failure),
            "denied" => Some(Self::Denied),
            _ => None,
        }
    }
}

impl From<AppendEventRequestOutcome> for AuditOutcome {
    fn from(value: AppendEventRequestOutcome) -> Self {
        match value {
            AppendEventRequestOutcome::Success => Self::Success,
            AppendEventRequestOutcome::Failure => Self::Failure,
            AppendEventRequestOutcome::Denied => Self::Denied,
        }
    }
}

impl From<ListEventsRequestOutcome> for AuditOutcome {
    fn from(value: ListEventsRequestOutcome) -> Self {
        match value {
            ListEventsRequestOutcome::Success => Self::Success,
            ListEventsRequestOutcome::Failure => Self::Failure,
            ListEventsRequestOutcome::Denied => Self::Denied,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditSeverity {
    Info,
    Warning,
    Critical,
}

impl AuditSeverity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "info" => Some(Self::Info),
            "warning" => Some(Self::Warning),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }
}

impl From<AppendEventRequestSeverity> for AuditSeverity {
    fn from(value: AppendEventRequestSeverity) -> Self {
        match value {
            AppendEventRequestSeverity::Info => Self::Info,
            AppendEventRequestSeverity::Warning => Self::Warning,
            AppendEventRequestSeverity::Critical => Self::Critical,
        }
    }
}

impl From<ListEventsRequestSeverity> for AuditSeverity {
    fn from(value: ListEventsRequestSeverity) -> Self {
        match value {
            ListEventsRequestSeverity::Info => Self::Info,
            ListEventsRequestSeverity::Warning => Self::Warning,
            ListEventsRequestSeverity::Critical => Self::Critical,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NewAuditEvent {
    pub id: String,
    pub event_name: String,
    pub source_instance: String,
    pub action: String,
    pub outcome: AuditOutcome,
    pub severity: AuditSeverity,
    pub actor_kind: String,
    pub actor_id: Option<String>,
    pub actor_display: Option<String>,
    pub scope_module: Option<String>,
    pub scope_type: Option<String>,
    pub scope_id: Option<String>,
    pub scope_display: Option<String>,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub resource_display: Option<String>,
    pub correlation_id: Option<String>,
    pub causation_id: Option<String>,
    pub request_id: Option<String>,
    pub story_id: Option<String>,
    pub reason: Option<String>,
    pub metadata: BTreeMap<String, Value>,
    pub occurred_at: DateTime<Utc>,
}

impl NewAuditEvent {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_request(
        request: AppendEventRequest,
        source_instance: &str,
    ) -> Result<Self, AppendEventError> {
        Self::from_request_with_id(request, source_instance, || {
            format!("audit_evt_{}", Uuid::now_v7())
        })
    }

    pub fn from_request_with_id(
        request: AppendEventRequest,
        source_instance: &str,
        fresh_id: impl FnOnce() -> String,
    ) -> Result<Self, AppendEventError> {
        if request.idempotency_key.as_deref().is_some_and(|key| {
            key.is_empty()
                || key.len() > 256
                || !key.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-')
                })
        }) || !valid_required(&request.event_name, 256)
            || !valid_required(source_instance, 256)
            || !valid_required(&request.action, 128)
            || !valid_required(&request.actor.kind, 128)
            || !valid_optional(request.actor.id.as_deref(), 512)
            || !valid_optional(request.actor.display.as_deref(), 512)
            || !valid_optional(request.reason.as_deref(), 2_048)
        {
            return Err(AppendEventError::InvalidEvent);
        }
        if let Some(scope) = &request.scope
            && (!valid_optional(scope.module.as_deref(), 128)
                || !valid_required(&scope.scope_type, 128)
                || !valid_required(&scope.id, 512)
                || !valid_optional(scope.display.as_deref(), 512))
        {
            return Err(AppendEventError::InvalidEvent);
        }
        if let Some(resource) = &request.resource
            && (!valid_required(&resource.resource_type, 128)
                || !valid_required(&resource.id, 512)
                || !valid_optional(resource.display.as_deref(), 512))
        {
            return Err(AppendEventError::InvalidEvent);
        }
        if let Some(context) = &request.request_context
            && (!valid_optional(context.correlation_id.as_deref(), 512)
                || !valid_optional(context.causation_id.as_deref(), 512)
                || !valid_optional(context.request_id.as_deref(), 512)
                || !valid_optional(context.story_id.as_deref(), 512))
        {
            return Err(AppendEventError::InvalidEvent);
        }
        let metadata_value = Value::Object(request.metadata.clone().into_iter().collect());
        if !metadata_within_wire_bounds(&metadata_value) {
            return Err(AppendEventError::InvalidEvent);
        }
        let occurred_at =
            parse_timestamp(&request.occurred_at).ok_or(AppendEventError::InvalidEvent)?;
        let metadata = match redact_metadata(metadata_value) {
            Value::Object(metadata) => metadata.into_iter().collect(),
            _ => unreachable!("Audit metadata starts as an object"),
        };
        let scope = request.scope;
        let resource = request.resource;
        let context = request.request_context;
        Ok(Self {
            id: request
                .idempotency_key
                .as_deref()
                .map_or_else(fresh_id, |key| stable_event_id(source_instance, key)),
            event_name: request.event_name,
            source_instance: source_instance.to_owned(),
            action: request.action,
            outcome: request.outcome.into(),
            severity: request.severity.into(),
            actor_kind: request.actor.kind,
            actor_id: request.actor.id,
            actor_display: request.actor.display,
            scope_module: scope.as_ref().and_then(|value| value.module.clone()),
            scope_type: scope.as_ref().map(|value| value.scope_type.clone()),
            scope_id: scope.as_ref().map(|value| value.id.clone()),
            scope_display: scope.and_then(|value| value.display),
            resource_type: resource.as_ref().map(|value| value.resource_type.clone()),
            resource_id: resource.as_ref().map(|value| value.id.clone()),
            resource_display: resource.and_then(|value| value.display),
            correlation_id: context
                .as_ref()
                .and_then(|value| value.correlation_id.clone()),
            causation_id: context
                .as_ref()
                .and_then(|value| value.causation_id.clone()),
            request_id: context.as_ref().and_then(|value| value.request_id.clone()),
            story_id: context.and_then(|value| value.story_id),
            reason: request.reason,
            metadata,
            occurred_at,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventFilter {
    pub event_name: Option<String>,
    pub source_instance: Option<String>,
    pub outcome: Option<AuditOutcome>,
    pub severity: Option<AuditSeverity>,
    pub actor_kind: Option<String>,
    pub actor_id: Option<String>,
    pub scope_module: Option<String>,
    pub scope_type: Option<String>,
    pub scope_id: Option<String>,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub correlation_id: Option<String>,
    pub occurred_after: Option<DateTime<Utc>>,
    pub occurred_before: Option<DateTime<Utc>>,
    pub cursor: Option<EventCursor>,
    pub limit: i64,
}

impl EventFilter {
    pub fn from_request(request: ListEventsRequest) -> Result<Self, ListEventsError> {
        let valid_filters = [
            (request.event_name.as_deref(), 256),
            (request.source_instance.as_deref(), 256),
            (request.actor_kind.as_deref(), 128),
            (request.actor_id.as_deref(), 512),
            (request.scope_module.as_deref(), 128),
            (request.scope_type.as_deref(), 128),
            (request.scope_id.as_deref(), 512),
            (request.resource_type.as_deref(), 128),
            (request.resource_id.as_deref(), 512),
            (request.correlation_id.as_deref(), 512),
        ]
        .into_iter()
        .all(|(value, maximum)| valid_optional(value, maximum));
        if !valid_filters || !(1..=200).contains(&request.limit) {
            return Err(ListEventsError::InvalidQuery);
        }
        let occurred_after = match request.occurred_after.as_deref() {
            Some(value) => Some(parse_timestamp(value).ok_or(ListEventsError::InvalidQuery)?),
            None => None,
        };
        let occurred_before = match request.occurred_before.as_deref() {
            Some(value) => Some(parse_timestamp(value).ok_or(ListEventsError::InvalidQuery)?),
            None => None,
        };
        if occurred_after
            .zip(occurred_before)
            .is_some_and(|(after, before)| after > before)
        {
            return Err(ListEventsError::InvalidQuery);
        }
        let cursor = match request.cursor {
            Some(cursor) => {
                if !valid_required(&cursor.id, 512) {
                    return Err(ListEventsError::InvalidQuery);
                }
                Some(EventCursor {
                    occurred_at: parse_timestamp(&cursor.occurred_at)
                        .ok_or(ListEventsError::InvalidQuery)?,
                    id: cursor.id,
                })
            }
            None => None,
        };
        Ok(Self {
            event_name: request.event_name,
            source_instance: request.source_instance,
            outcome: request.outcome.map(Into::into),
            severity: request.severity.map(Into::into),
            actor_kind: request.actor_kind,
            actor_id: request.actor_id,
            scope_module: request.scope_module,
            scope_type: request.scope_type,
            scope_id: request.scope_id,
            resource_type: request.resource_type,
            resource_id: request.resource_id,
            correlation_id: request.correlation_id,
            occurred_after,
            occurred_before,
            cursor,
            limit: request.limit,
        })
    }

    pub fn matches(&self, event: &StoredEvent) -> bool {
        matches_optional(self.event_name.as_ref(), &event.event_name)
            && matches_optional(self.source_instance.as_ref(), &event.source_instance)
            && self.outcome.is_none_or(|value| value == event.outcome)
            && self.severity.is_none_or(|value| value == event.severity)
            && matches_optional(self.actor_kind.as_ref(), &event.actor_kind)
            && matches_optional_option(self.actor_id.as_ref(), event.actor_id.as_ref())
            && matches_optional_option(self.scope_module.as_ref(), event.scope_module.as_ref())
            && matches_optional_option(self.scope_type.as_ref(), event.scope_type.as_ref())
            && matches_optional_option(self.scope_id.as_ref(), event.scope_id.as_ref())
            && matches_optional_option(self.resource_type.as_ref(), event.resource_type.as_ref())
            && matches_optional_option(self.resource_id.as_ref(), event.resource_id.as_ref())
            && matches_optional_option(self.correlation_id.as_ref(), event.correlation_id.as_ref())
            && self
                .occurred_after
                .is_none_or(|value| event.occurred_at >= value)
            && self
                .occurred_before
                .is_none_or(|value| event.occurred_at <= value)
            && self.cursor.as_ref().is_none_or(|cursor| {
                event.occurred_at < cursor.occurred_at
                    || (event.occurred_at == cursor.occurred_at && event.id < cursor.id)
            })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventCursor {
    pub occurred_at: DateTime<Utc>,
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredEvent {
    pub id: String,
    pub event_name: String,
    pub source_instance: String,
    pub action: String,
    pub outcome: AuditOutcome,
    pub severity: AuditSeverity,
    pub actor_kind: String,
    pub actor_id: Option<String>,
    pub actor_display: Option<String>,
    pub scope_module: Option<String>,
    pub scope_type: Option<String>,
    pub scope_id: Option<String>,
    pub scope_display: Option<String>,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub resource_display: Option<String>,
    pub correlation_id: Option<String>,
    pub causation_id: Option<String>,
    pub request_id: Option<String>,
    pub story_id: Option<String>,
    pub reason: Option<String>,
    pub metadata: BTreeMap<String, Value>,
    pub occurred_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

impl StoredEvent {
    pub fn from_event(event: NewAuditEvent, created_at: DateTime<Utc>) -> Self {
        Self {
            id: event.id,
            event_name: event.event_name,
            source_instance: event.source_instance,
            action: event.action,
            outcome: event.outcome,
            severity: event.severity,
            actor_kind: event.actor_kind,
            actor_id: event.actor_id,
            actor_display: event.actor_display,
            scope_module: event.scope_module,
            scope_type: event.scope_type,
            scope_id: event.scope_id,
            scope_display: event.scope_display,
            resource_type: event.resource_type,
            resource_id: event.resource_id,
            resource_display: event.resource_display,
            correlation_id: event.correlation_id,
            causation_id: event.causation_id,
            request_id: event.request_id,
            story_id: event.story_id,
            reason: event.reason,
            metadata: event.metadata,
            occurred_at: event.occurred_at,
            created_at,
        }
    }

    pub fn project<T: DeserializeOwned>(&self) -> Result<T, ProjectionError> {
        self.validate_wire_projection()?;
        serde_json::to_value(self)
            .and_then(serde_json::from_value)
            .map_err(ProjectionError::Serialization)
    }

    fn validate_wire_projection(&self) -> Result<(), ProjectionError> {
        if !valid_required(&self.id, 512) {
            return Err(ProjectionError::InvalidStoredValue { field: "id" });
        }
        if !valid_required(&self.event_name, 256) {
            return Err(ProjectionError::InvalidStoredValue {
                field: "event_name",
            });
        }
        if !valid_required(&self.source_instance, 256) {
            return Err(ProjectionError::InvalidStoredValue {
                field: "source_instance",
            });
        }
        if !valid_required(&self.action, 128) {
            return Err(ProjectionError::InvalidStoredValue { field: "action" });
        }
        if !valid_required(&self.actor_kind, 128) {
            return Err(ProjectionError::InvalidStoredValue {
                field: "actor_kind",
            });
        }
        for (field, value, maximum) in [
            ("actor_id", self.actor_id.as_deref(), 512),
            ("actor_display", self.actor_display.as_deref(), 512),
            ("scope_module", self.scope_module.as_deref(), 128),
            ("scope_type", self.scope_type.as_deref(), 128),
            ("scope_id", self.scope_id.as_deref(), 512),
            ("scope_display", self.scope_display.as_deref(), 512),
            ("resource_type", self.resource_type.as_deref(), 128),
            ("resource_id", self.resource_id.as_deref(), 512),
            ("resource_display", self.resource_display.as_deref(), 512),
            ("correlation_id", self.correlation_id.as_deref(), 512),
            ("causation_id", self.causation_id.as_deref(), 512),
            ("request_id", self.request_id.as_deref(), 512),
            ("story_id", self.story_id.as_deref(), 512),
            ("reason", self.reason.as_deref(), 2_048),
        ] {
            if !valid_optional(value, maximum) {
                return Err(ProjectionError::InvalidStoredValue { field });
            }
        }
        if !valid_wire_timestamp(&self.occurred_at) {
            return Err(ProjectionError::InvalidStoredValue {
                field: "occurred_at",
            });
        }
        if !valid_wire_timestamp(&self.created_at) {
            return Err(ProjectionError::InvalidStoredValue {
                field: "created_at",
            });
        }
        let metadata = Value::Object(self.metadata.clone().into_iter().collect());
        if !metadata_within_wire_bounds(&metadata) {
            return Err(ProjectionError::InvalidStoredValue { field: "metadata" });
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ProjectionError {
    #[error("stored Audit Event field `{field}` is outside the Capability wire boundary")]
    InvalidStoredValue { field: &'static str },
    #[error("serialize bounded Audit Event projection")]
    Serialization(#[source] serde_json::Error),
}

fn stable_event_id(source: &str, key: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(
        u64::try_from(source.len())
            .expect("bounded source length")
            .to_be_bytes(),
    );
    digest.update(source.as_bytes());
    digest.update(
        u64::try_from(key.len())
            .expect("bounded delivery key length")
            .to_be_bytes(),
    );
    digest.update(key.as_bytes());
    format!("audit_evt_{:x}", digest.finalize())
}

pub fn validate_event_id(id: &str) -> Result<(), GetEventError> {
    if valid_required(id, 512) {
        Ok(())
    } else {
        Err(GetEventError::InvalidId)
    }
}

pub fn redact_metadata(value: Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(redact_object(object)),
        Value::Array(items) => Value::Array(items.into_iter().map(redact_metadata).collect()),
        value => value,
    }
}

fn redact_object(object: Map<String, Value>) -> Map<String, Value> {
    object
        .into_iter()
        .map(|(key, value)| {
            let value = if is_sensitive_key(&key) {
                Value::String("[redacted]".to_owned())
            } else {
                redact_metadata(value)
            };
            (key, value)
        })
        .collect()
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| !matches!(character, '_' | '-' | ' '))
        .flat_map(char::to_lowercase)
        .collect::<String>();
    [
        "password",
        "token",
        "secret",
        "privatekey",
        "apikey",
        "authorization",
        "cookie",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn valid_wire_timestamp(value: &DateTime<Utc>) -> bool {
    (0..=9_999).contains(&value.year()) && value.to_rfc3339().chars().count() <= MAX_TIMESTAMP_CHARS
}

fn valid_required(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.chars().count() <= maximum && !value.chars().any(char::is_control)
}

fn valid_optional(value: Option<&str>, maximum: usize) -> bool {
    value.is_none_or(|value| valid_required(value, maximum))
}

fn metadata_within_wire_bounds(metadata: &Value) -> bool {
    let mut pending = vec![(metadata, 0_usize)];
    let mut nodes = 0_usize;
    while let Some((value, depth)) = pending.pop() {
        nodes = nodes.saturating_add(1);
        if nodes > MAX_METADATA_NODES || depth > MAX_METADATA_DEPTH {
            return false;
        }
        match value {
            Value::Object(object) => {
                if object.len() > MAX_METADATA_CONTAINER_ITEMS
                    || object
                        .keys()
                        .any(|key| key.chars().count() > MAX_METADATA_KEY_CHARS)
                {
                    return false;
                }
                pending.extend(
                    object
                        .values()
                        .map(|value| (value, depth.saturating_add(1))),
                );
            }
            Value::Array(items) => {
                if items.len() > MAX_METADATA_CONTAINER_ITEMS {
                    return false;
                }
                pending.extend(items.iter().map(|value| (value, depth.saturating_add(1))));
            }
            Value::String(value) if value.chars().count() > MAX_METADATA_STRING_CHARS => {
                return false;
            }
            _ => {}
        }
    }

    let mut counter = BoundedJsonCounter::new(MAX_METADATA_BYTES);
    lenso_contract_runtime::validate_portable_json_value(metadata).is_ok()
        && serde_json::to_writer(&mut counter, metadata).is_ok()
}

fn matches_optional(filter: Option<&String>, value: &str) -> bool {
    filter.is_none_or(|filter| filter == value)
}

fn matches_optional_option(filter: Option<&String>, value: Option<&String>) -> bool {
    filter.is_none_or(|filter| value == Some(filter))
}
