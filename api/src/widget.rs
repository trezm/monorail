//! Sample domain type and its storage port.
//!
//! This exists to show the shape of a resource end to end — model, validation,
//! repository, HTTP handlers, tests. Replace it with real domain types; the
//! parts worth keeping are the trait-free [`WidgetStore`] seam and the
//! validation-returns-[`ApiError`] convention.

use std::{
    collections::HashMap,
    sync::{RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};

const NAME_MAX_LEN: usize = 100;
const DESCRIPTION_MAX_LEN: usize = 2_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Widget {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request body for creating a widget.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewWidget {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Request body for a partial update. An absent field is left unchanged; an
/// explicit `null` description clears it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    pub description: Option<Option<String>>,
}

/// Distinguishes `{"description": null}` from an absent `description`.
fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

fn validate_name(name: &str) -> ApiResult<String> {
    let name = name.trim();

    if name.is_empty() {
        return Err(ApiError::UnprocessableEntity(
            "name must not be blank".into(),
        ));
    }
    if name.chars().count() > NAME_MAX_LEN {
        return Err(ApiError::UnprocessableEntity(format!(
            "name must be at most {NAME_MAX_LEN} characters"
        )));
    }

    Ok(name.to_owned())
}

fn validate_description(description: Option<String>) -> ApiResult<Option<String>> {
    let Some(description) = description else {
        return Ok(None);
    };

    if description.chars().count() > DESCRIPTION_MAX_LEN {
        return Err(ApiError::UnprocessableEntity(format!(
            "description must be at most {DESCRIPTION_MAX_LEN} characters"
        )));
    }

    Ok(Some(description))
}

/// In-memory stand-in for a real repository.
///
/// Deliberately not behind a trait: add the trait when there is a second
/// implementation to justify it. When this becomes a database, the method
/// signatures already return [`ApiResult`], so callers do not change.
///
/// The lock is a `std::sync::RwLock`, not a `tokio::sync::RwLock`, because no
/// guard is ever held across an `.await`.
#[derive(Debug, Default)]
pub struct WidgetStore {
    widgets: RwLock<HashMap<Uuid, Widget>>,
}

impl WidgetStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn read(&self) -> RwLockReadGuard<'_, HashMap<Uuid, Widget>> {
        self.widgets.read().unwrap_or_else(|poisoned| {
            self.widgets.clear_poison();
            poisoned.into_inner()
        })
    }

    fn write(&self) -> RwLockWriteGuard<'_, HashMap<Uuid, Widget>> {
        self.widgets.write().unwrap_or_else(|poisoned| {
            self.widgets.clear_poison();
            poisoned.into_inner()
        })
    }

    /// Returns a page of widgets in stable creation order, plus the total count.
    pub fn list(&self, limit: usize, offset: usize) -> ApiResult<(Vec<Widget>, usize)> {
        let widgets = self.read();

        let mut all: Vec<Widget> = widgets.values().cloned().collect();
        all.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));

        let total = all.len();
        let page = all.into_iter().skip(offset).take(limit).collect();

        Ok((page, total))
    }

    pub fn get(&self, id: Uuid) -> ApiResult<Widget> {
        self.read()
            .get(&id)
            .cloned()
            .ok_or_else(|| ApiError::not_found("widget", id))
    }

    pub fn create(&self, new: NewWidget) -> ApiResult<Widget> {
        let name = validate_name(&new.name)?;
        let description = validate_description(new.description)?;

        let now = Utc::now();
        let widget = Widget {
            id: Uuid::new_v4(),
            name,
            description,
            created_at: now,
            updated_at: now,
        };

        self.write().insert(widget.id, widget.clone());

        Ok(widget)
    }

    pub fn update(&self, id: Uuid, patch: WidgetPatch) -> ApiResult<Widget> {
        // Validate before taking the write lock so a bad request cannot hold it.
        let name = patch.name.as_deref().map(validate_name).transpose()?;
        let replaces_description = patch.description.is_some();
        let description = patch
            .description
            .map(validate_description)
            .transpose()?
            .flatten();

        let mut widgets = self.write();
        let widget = widgets
            .get_mut(&id)
            .ok_or_else(|| ApiError::not_found("widget", id))?;

        if let Some(name) = name {
            widget.name = name;
        }
        if replaces_description {
            widget.description = description;
        }
        widget.updated_at = Utc::now();

        Ok(widget.clone())
    }

    pub fn delete(&self, id: Uuid) -> ApiResult<()> {
        self.write()
            .remove(&id)
            .map(|_| ())
            .ok_or_else(|| ApiError::not_found("widget", id))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.read().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_widget(name: &str) -> NewWidget {
        NewWidget {
            name: name.to_owned(),
            description: None,
        }
    }

    #[test]
    fn rejects_blank_and_overlong_names() {
        let store = WidgetStore::new();

        assert!(store.create(new_widget("   ")).is_err());
        assert!(
            store
                .create(new_widget(&"a".repeat(NAME_MAX_LEN + 1)))
                .is_err()
        );
        assert!(store.create(new_widget(&"a".repeat(NAME_MAX_LEN))).is_ok());
    }

    #[test]
    fn trims_names_on_create() {
        let store = WidgetStore::new();
        let widget = store.create(new_widget("  spaced  ")).unwrap();

        assert_eq!(widget.name, "spaced");
    }

    #[test]
    fn patch_leaves_absent_fields_untouched() {
        let store = WidgetStore::new();
        let created = store
            .create(NewWidget {
                name: "original".into(),
                description: Some("keep me".into()),
            })
            .unwrap();

        let updated = store
            .update(
                created.id,
                WidgetPatch {
                    name: Some("renamed".into()),
                    description: None,
                },
            )
            .unwrap();

        assert_eq!(updated.name, "renamed");
        assert_eq!(updated.description.as_deref(), Some("keep me"));
    }

    #[test]
    fn explicit_null_clears_description() {
        let store = WidgetStore::new();
        let created = store
            .create(NewWidget {
                name: "original".into(),
                description: Some("clear me".into()),
            })
            .unwrap();

        let updated = store
            .update(
                created.id,
                WidgetPatch {
                    name: None,
                    description: Some(None),
                },
            )
            .unwrap();

        assert_eq!(updated.description, None);
    }

    #[test]
    fn list_paginates_in_creation_order() {
        let store = WidgetStore::new();
        for i in 0..5 {
            store.create(new_widget(&format!("w{i}"))).unwrap();
        }

        let (page, total) = store.list(2, 1).unwrap();

        assert_eq!(total, 5);
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].name, "w1");
        assert_eq!(page[1].name, "w2");
    }

    #[test]
    fn missing_ids_are_not_found() {
        let store = WidgetStore::new();
        let id = Uuid::new_v4();

        assert!(matches!(store.get(id), Err(ApiError::NotFound(_))));
        assert!(matches!(store.delete(id), Err(ApiError::NotFound(_))));
        assert!(matches!(
            store.update(id, WidgetPatch::default()),
            Err(ApiError::NotFound(_))
        ));
    }
}
