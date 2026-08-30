use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use lenso_capability_audit_log::{LEGACY_METADATA_PORTABLE_JSON_KEY, LEGACY_METADATA_VALUE_KEY};
use lenso_postgres_kit::OwnedPostgres;
use serde_json::Value;
use sqlx::{Postgres, QueryBuilder, Row, postgres::PgRow};
use thiserror::Error;

use crate::model::{AuditOutcome, AuditSeverity, EventFilter, NewAuditEvent, StoredEvent};

const LIST_EVENTS_SQL: &str = r"
    select
        id,
        event_name,
        module_name,
        action,
        outcome,
        severity,
        actor_kind,
        actor_id,
        actor_display,
        scope_module,
        scope_type,
        scope_id,
        scope_display,
        resource_type,
        resource_id,
        resource_display,
        correlation_id,
        causation_id,
        request_id,
        story_id,
        reason,
        metadata,
        occurred_at,
        created_at
    from audit_log.events
    ";

const GET_EVENT_SQL: &str = r"
    select
        id,
        event_name,
        module_name,
        action,
        outcome,
        severity,
        actor_kind,
        actor_id,
        actor_display,
        scope_module,
        scope_type,
        scope_id,
        scope_display,
        resource_type,
        resource_id,
        resource_display,
        correlation_id,
        causation_id,
        request_id,
        story_id,
        reason,
        metadata,
        occurred_at,
        created_at
    from audit_log.events
    where id = $1
    ";

const INSERT_EVENT_SQL: &str = r"
    insert into audit_log.events (
        id,
        event_name,
        module_name,
        action,
        outcome,
        severity,
        actor_kind,
        actor_id,
        actor_display,
        scope_module,
        scope_type,
        scope_id,
        scope_display,
        resource_type,
        resource_id,
        resource_display,
        correlation_id,
        causation_id,
        request_id,
        story_id,
        reason,
        metadata,
        occurred_at
    )
    values (
        $1,
        $2,
        $3,
        $4,
        $5,
        $6,
        $7,
        $8,
        $9,
        $10,
        $11,
        $12,
        $13,
        $14,
        $15,
        $16,
        $17,
        $18,
        $19,
        $20,
        $21,
        $22,
        $23
    )
    returning
        id,
        event_name,
        module_name,
        action,
        outcome,
        severity,
        actor_kind,
        actor_id,
        actor_display,
        scope_module,
        scope_type,
        scope_id,
        scope_display,
        resource_type,
        resource_id,
        resource_display,
        correlation_id,
        causation_id,
        request_id,
        story_id,
        reason,
        metadata,
        occurred_at,
        created_at
    ";

pub(crate) async fn append_event(
    postgres: &OwnedPostgres,
    event: NewAuditEvent,
) -> Result<StoredEvent, RepositoryError> {
    let metadata = Value::Object(event.metadata.clone().into_iter().collect());
    let row = sqlx::query(INSERT_EVENT_SQL)
        .bind(event.id)
        .bind(event.event_name)
        .bind(event.source_instance)
        .bind(event.action)
        .bind(event.outcome.as_str())
        .bind(event.severity.as_str())
        .bind(event.actor_kind)
        .bind(event.actor_id)
        .bind(event.actor_display)
        .bind(event.scope_module)
        .bind(event.scope_type)
        .bind(event.scope_id)
        .bind(event.scope_display)
        .bind(event.resource_type)
        .bind(event.resource_id)
        .bind(event.resource_display)
        .bind(event.correlation_id)
        .bind(event.causation_id)
        .bind(event.request_id)
        .bind(event.story_id)
        .bind(event.reason)
        .bind(metadata)
        .bind(event.occurred_at)
        .fetch_one(postgres.pool())
        .await?;
    map_event_row(&row)
}

pub(crate) async fn list_events(
    postgres: &OwnedPostgres,
    filter: &EventFilter,
) -> Result<Vec<StoredEvent>, RepositoryError> {
    let mut builder = QueryBuilder::<Postgres>::new(LIST_EVENTS_SQL);
    push_event_filters(&mut builder, filter);
    builder
        .push(" order by occurred_at desc, id desc limit ")
        .push_bind(filter.limit.saturating_add(1));
    let rows = builder.build().fetch_all(postgres.pool()).await?;
    rows.iter().map(map_event_row).collect()
}

pub(crate) async fn get_event(
    postgres: &OwnedPostgres,
    id: &str,
) -> Result<Option<StoredEvent>, RepositoryError> {
    let row = sqlx::query(GET_EVENT_SQL)
        .bind(id)
        .fetch_optional(postgres.pool())
        .await?;
    row.as_ref().map(map_event_row).transpose()
}

fn push_event_filters(builder: &mut QueryBuilder<Postgres>, filter: &EventFilter) {
    let mut has_where = false;
    push_text_filter(
        builder,
        &mut has_where,
        "event_name",
        filter.event_name.as_deref(),
    );
    push_text_filter(
        builder,
        &mut has_where,
        "module_name",
        filter.source_instance.as_deref(),
    );
    if let Some(outcome) = filter.outcome {
        push_where(builder, &mut has_where);
        builder.push("outcome = ").push_bind(outcome.as_str());
    }
    if let Some(severity) = filter.severity {
        push_where(builder, &mut has_where);
        builder.push("severity = ").push_bind(severity.as_str());
    }
    push_text_filter(
        builder,
        &mut has_where,
        "actor_kind",
        filter.actor_kind.as_deref(),
    );
    push_text_filter(
        builder,
        &mut has_where,
        "actor_id",
        filter.actor_id.as_deref(),
    );
    push_text_filter(
        builder,
        &mut has_where,
        "scope_module",
        filter.scope_module.as_deref(),
    );
    push_text_filter(
        builder,
        &mut has_where,
        "scope_type",
        filter.scope_type.as_deref(),
    );
    push_text_filter(
        builder,
        &mut has_where,
        "scope_id",
        filter.scope_id.as_deref(),
    );
    push_text_filter(
        builder,
        &mut has_where,
        "resource_type",
        filter.resource_type.as_deref(),
    );
    push_text_filter(
        builder,
        &mut has_where,
        "resource_id",
        filter.resource_id.as_deref(),
    );
    push_text_filter(
        builder,
        &mut has_where,
        "correlation_id",
        filter.correlation_id.as_deref(),
    );
    if let Some(occurred_after) = filter.occurred_after {
        push_where(builder, &mut has_where);
        builder.push("occurred_at >= ").push_bind(occurred_after);
    }
    if let Some(occurred_before) = filter.occurred_before {
        push_where(builder, &mut has_where);
        builder.push("occurred_at <= ").push_bind(occurred_before);
    }
    if let Some(cursor) = &filter.cursor {
        push_where(builder, &mut has_where);
        builder
            .push("(occurred_at < ")
            .push_bind(cursor.occurred_at)
            .push(" or (occurred_at = ")
            .push_bind(cursor.occurred_at)
            .push(" and id < ")
            .push_bind(cursor.id.clone())
            .push("))");
    }
}

fn push_text_filter(
    builder: &mut QueryBuilder<Postgres>,
    has_where: &mut bool,
    column: &'static str,
    value: Option<&str>,
) {
    if let Some(value) = value {
        push_where(builder, has_where);
        builder.push(column).push(" = ").push_bind(value.to_owned());
    }
}

fn push_where(builder: &mut QueryBuilder<Postgres>, has_where: &mut bool) {
    if *has_where {
        builder.push(" and ");
    } else {
        builder.push(" where ");
        *has_where = true;
    }
}

fn map_event_row(row: &PgRow) -> Result<StoredEvent, RepositoryError> {
    let outcome = row.try_get::<String, _>("outcome")?;
    let severity = row.try_get::<String, _>("severity")?;
    let metadata = project_stored_metadata(row.try_get::<Value, _>("metadata")?);
    Ok(StoredEvent {
        id: row.try_get("id")?,
        event_name: row.try_get("event_name")?,
        source_instance: row.try_get("module_name")?,
        action: row.try_get("action")?,
        outcome: AuditOutcome::parse(&outcome)
            .ok_or(RepositoryError::InvalidStoredValue { field: "outcome" })?,
        severity: AuditSeverity::parse(&severity)
            .ok_or(RepositoryError::InvalidStoredValue { field: "severity" })?,
        actor_kind: row.try_get("actor_kind")?,
        actor_id: row.try_get("actor_id")?,
        actor_display: row.try_get("actor_display")?,
        scope_module: row.try_get("scope_module")?,
        scope_type: row.try_get("scope_type")?,
        scope_id: row.try_get("scope_id")?,
        scope_display: row.try_get("scope_display")?,
        resource_type: row.try_get("resource_type")?,
        resource_id: row.try_get("resource_id")?,
        resource_display: row.try_get("resource_display")?,
        correlation_id: row.try_get("correlation_id")?,
        causation_id: row.try_get("causation_id")?,
        request_id: row.try_get("request_id")?,
        story_id: row.try_get("story_id")?,
        reason: row.try_get("reason")?,
        metadata: metadata.into_iter().collect(),
        occurred_at: row.try_get::<DateTime<Utc>, _>("occurred_at")?,
        created_at: row.try_get::<DateTime<Utc>, _>("created_at")?,
    })
}

pub(crate) fn project_stored_metadata(metadata: Value) -> BTreeMap<String, Value> {
    let is_portable = lenso_contract_runtime::validate_portable_json_value(&metadata).is_ok();
    if let Value::Object(object) = &metadata
        && is_portable
        && !object.contains_key(LEGACY_METADATA_VALUE_KEY)
        && !object.contains_key(LEGACY_METADATA_PORTABLE_JSON_KEY)
    {
        return object.clone().into_iter().collect();
    }

    if is_portable {
        BTreeMap::from([(LEGACY_METADATA_VALUE_KEY.to_owned(), metadata)])
    } else {
        // `serde_json::Value` always serializes. Keeping the original JSON text behind a string
        // makes the wire portable without rounding or otherwise changing an historical number.
        let encoded = serde_json::to_string(&metadata)
            .expect("a decoded PostgreSQL JSON value always serializes as JSON");
        BTreeMap::from([(
            LEGACY_METADATA_PORTABLE_JSON_KEY.to_owned(),
            Value::String(encoded),
        )])
    }
}

#[derive(Debug, Error)]
pub(crate) enum RepositoryError {
    #[error("PostgreSQL audit operation failed")]
    Database(#[from] sqlx::Error),
    #[error("stored Audit Event field `{field}` is invalid")]
    InvalidStoredValue { field: &'static str },
}
