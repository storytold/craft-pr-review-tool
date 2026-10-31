//! What's in Craft Status's own snapshot (never written here): app icons, open-PR
//! counts to show before this app's first fetch, and whether it has spotted a PR
//! newer than this app's list. What the apps fetch in common is in
//! `craft_core::shared`.

use std::collections::BTreeMap;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use craft_core::Cache;
use serde::Serialize;

use crate::model::Time;

/// Readings older than this say nothing about the current budget.
const BUDGET_FRESH_MINUTES: i64 = 10;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SiblingRepo {
    pub icon: Option<String>,
    pub open_prs: Option<u64>,
    pub newest_pr_at: Option<Time>,
    pub fetched_at: Option<Time>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sibling {
    pub present: bool,
    pub saved_at: Option<Time>,
    pub remaining: Option<u64>,
    pub repos: BTreeMap<String, SiblingRepo>,
    mtime: Option<SystemTime>,
}

impl Sibling {
    /// Re-reads Craft Status's snapshot if it changed since the last call.
    pub fn refresh(&mut self) {
        let Ok(cache) = Cache::default_location() else {
            return;
        };
        let mtime = std::fs::metadata(cache.path())
            .ok()
            .and_then(|m| m.modified().ok());
        if mtime == self.mtime {
            return;
        }
        self.mtime = mtime;
        let Some(snap) = cache.load() else {
            *self = Sibling {
                mtime,
                ..Default::default()
            };
            return;
        };
        self.present = true;
        self.saved_at = mtime.map(DateTime::<Utc>::from);
        self.remaining = snap.rate_limit_remaining;
        self.repos = snap
            .repos
            .into_iter()
            .map(|e| {
                let info = SiblingRepo {
                    icon: e.icon,
                    open_prs: e.stats.as_ref().map(|s| s.open_prs),
                    newest_pr_at: e.stats.as_ref().and_then(|s| s.newest_pr_at),
                    fetched_at: e.fetched_at,
                };
                (e.repo, info)
            })
            .collect();
    }

    /// The GraphQL points left when Craft Status last wrote, if that was recently.
    pub fn fresh_remaining(&self, now: Time) -> Option<u64> {
        self.remaining.filter(|_| {
            self.saved_at
                .is_some_and(|t| (now - t).num_minutes() < BUDGET_FRESH_MINUTES)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_budget_readings_are_ignored() {
        let now: Time = "2026-10-31T12:00:00Z".parse().unwrap();
        let mut s = Sibling {
            remaining: Some(900),
            saved_at: Some(now - chrono::Duration::minutes(3)),
            ..Default::default()
        };
        assert_eq!(s.fresh_remaining(now), Some(900));
        s.saved_at = Some(now - chrono::Duration::minutes(30));
        assert_eq!(s.fresh_remaining(now), None);
    }
}
