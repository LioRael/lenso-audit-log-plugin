use lenso_postgres_kit::OwnedPostgres;
use thiserror::Error;

use crate::{
    model::{EventFilter, NewAuditEvent, StoredEvent},
    repository::{self, RepositoryError},
};

#[derive(Clone, Debug)]
pub(crate) enum AuditStore {
    Postgres(OwnedPostgres),
    #[cfg(test)]
    Fixture(FixtureAuditStore),
}

impl AuditStore {
    pub(crate) async fn append_event(
        &self,
        event: NewAuditEvent,
    ) -> Result<StoredEvent, AuditStoreError> {
        match self {
            Self::Postgres(postgres) => Ok(repository::append_event(postgres, event).await?),
            #[cfg(test)]
            Self::Fixture(store) => store.append_event(event),
        }
    }

    pub(crate) async fn list_events(
        &self,
        filter: &EventFilter,
    ) -> Result<Vec<StoredEvent>, AuditStoreError> {
        match self {
            Self::Postgres(postgres) => Ok(repository::list_events(postgres, filter).await?),
            #[cfg(test)]
            Self::Fixture(store) => store.list_events(filter),
        }
    }

    pub(crate) async fn get_event(&self, id: &str) -> Result<Option<StoredEvent>, AuditStoreError> {
        match self {
            Self::Postgres(postgres) => Ok(repository::get_event(postgres, id).await?),
            #[cfg(test)]
            Self::Fixture(store) => store.get_event(id),
        }
    }

    pub(crate) async fn close(self) {
        match self {
            Self::Postgres(postgres) => postgres.pool().close().await,
            #[cfg(test)]
            Self::Fixture(_) => {}
        }
    }
}

#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub(crate) struct FixtureAuditStore {
    events: std::rc::Rc<std::cell::RefCell<Vec<StoredEvent>>>,
    fail: bool,
}

#[cfg(test)]
impl FixtureAuditStore {
    pub(crate) fn failing() -> Self {
        Self {
            events: std::rc::Rc::default(),
            fail: true,
        }
    }

    fn append_event(&self, event: NewAuditEvent) -> Result<StoredEvent, AuditStoreError> {
        self.ensure_available()?;
        let event = StoredEvent::fixture(event, chrono::Utc::now());
        self.events.borrow_mut().push(event.clone());
        Ok(event)
    }

    fn list_events(&self, filter: &EventFilter) -> Result<Vec<StoredEvent>, AuditStoreError> {
        self.ensure_available()?;
        let mut events = self
            .events
            .borrow()
            .iter()
            .filter(|event| filter.matches(event))
            .cloned()
            .collect::<Vec<_>>();
        events.sort_by(|left, right| {
            right
                .occurred_at
                .cmp(&left.occurred_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        let fetch_limit = usize::try_from(filter.limit.saturating_add(1)).unwrap_or(usize::MAX);
        events.truncate(fetch_limit);
        Ok(events)
    }

    fn get_event(&self, id: &str) -> Result<Option<StoredEvent>, AuditStoreError> {
        self.ensure_available()?;
        Ok(self
            .events
            .borrow()
            .iter()
            .find(|event| event.id == id)
            .cloned())
    }

    fn ensure_available(&self) -> Result<(), AuditStoreError> {
        if self.fail {
            Err(AuditStoreError::FixtureUnavailable)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum AuditStoreError {
    #[error(transparent)]
    Repository(#[from] RepositoryError),
    #[cfg(test)]
    #[error("fixture Audit storage is unavailable")]
    FixtureUnavailable,
}
