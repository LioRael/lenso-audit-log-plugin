create schema if not exists audit_log;

create table if not exists audit_log.events (
    id text primary key,
    event_name text not null,
    module_name text not null,
    action text not null,
    outcome text not null check (outcome in ('success', 'failure', 'denied')),
    severity text not null check (severity in ('info', 'warning', 'critical')),
    actor_kind text not null,
    actor_id text,
    actor_display text,
    scope_module text,
    scope_type text,
    scope_id text,
    scope_display text,
    resource_type text,
    resource_id text,
    resource_display text,
    correlation_id text,
    causation_id text,
    request_id text,
    story_id text,
    reason text,
    metadata jsonb not null default '{}'::jsonb,
    occurred_at timestamptz not null,
    created_at timestamptz not null default now()
);

create index if not exists audit_log_events_occurred_at_idx
    on audit_log.events (occurred_at desc, id desc);

create index if not exists audit_log_events_module_idx
    on audit_log.events (module_name, occurred_at desc, id desc);

create index if not exists audit_log_events_actor_idx
    on audit_log.events (actor_id, occurred_at desc, id desc)
    where actor_id is not null;

create index if not exists audit_log_events_scope_idx
    on audit_log.events (scope_type, scope_id, occurred_at desc, id desc)
    where scope_type is not null and scope_id is not null;

create index if not exists audit_log_events_resource_idx
    on audit_log.events (resource_type, resource_id, occurred_at desc, id desc)
    where resource_type is not null and resource_id is not null;

create index if not exists audit_log_events_correlation_idx
    on audit_log.events (correlation_id)
    where correlation_id is not null;
