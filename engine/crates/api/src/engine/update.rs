//! The update check: one GET to the releases API, kept in the home directory.

use super::{Engine, io};
use crate::convert;
use crate::error::{ApiError, ApiResult};
use crate::update::{self, Checked};
use crate::views::{AppInfo, CorpusCredits, UpdateStatus};

impl Engine {
    pub(crate) fn app_info_impl(&self) -> AppInfo {
        let credits = self.corpus.as_ref().map(|c| &c.manifest.credits);
        let clean = |s: &Option<String>| s.clone().filter(|s| !s.trim().is_empty());
        // Only a plain https address becomes a link.
        let link = |s: &Option<String>| {
            clean(s).filter(|u| {
                u.starts_with("https://") && !u.chars().any(|c| c.is_whitespace() || c.is_control())
            })
        };
        AppInfo {
            corpus_credits: credits
                .map(|c| CorpusCredits {
                    by: clean(&c.by),
                    url: link(&c.url),
                    project: link(&c.project)
                        .filter(|p| p.trim_end_matches('/') != update::REPOSITORY),
                    licence: clean(&c.licence),
                })
                .filter(|c| c.by.is_some() || c.licence.is_some()),
            ..AppInfo::current()
        }
    }

    fn status(&self, checked: Option<Checked>, offset_min: i32) -> UpdateStatus {
        UpdateStatus {
            current: update::VERSION.into(),
            newer: checked
                .as_ref()
                .is_some_and(|c| update::is_newer(&c.latest, update::VERSION)),
            checked_at: checked.as_ref().map(|c| convert::ts(c.at_ms, offset_min)),
            latest: checked.as_ref().map(|c| c.latest.clone()),
            url: checked.map(|c| c.url),
            auto: self.home.auto_update_check(),
        }
    }

    pub(crate) fn update_status_impl(&self) -> UpdateStatus {
        self.status(self.home.update_cache(), self.ports.clock.now().offset_min)
    }

    pub(crate) async fn check_update_impl(&self, force: bool) -> ApiResult<UpdateStatus> {
        let now = self.ports.clock.now();
        let cached = self.home.update_cache();
        let due = force
            || (self.home.auto_update_check()
                && !cached
                    .as_ref()
                    .is_some_and(|c| update::fresh(c.at_ms, now.ms)));
        if !due {
            return Ok(self.status(cached, now.offset_min));
        }
        let endpoint = |detail: String| ApiError::Endpoint { detail };
        let body = self
            .ports
            .net
            .get(update::LATEST_URL)
            .await
            .map_err(endpoint)?
            .ok_or_else(|| endpoint("no release found".into()))?;
        let (latest, url) = update::parse_release(&body).map_err(endpoint)?;
        let checked = Checked {
            at_ms: now.ms,
            latest,
            url,
        };
        self.home.set_update_cache(&checked).map_err(io)?;
        Ok(self.status(Some(checked), now.offset_min))
    }
}
