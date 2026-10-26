//! Endpoint profiles and roles: the file, the keychain and the endpoint-side lookups.

use super::{Engine, io};
use crate::endpoints::{Profile, Role, Roles, presets};
use crate::error::{ApiError, ApiResult};
use crate::views::*;

fn invalid(field: &str) -> ApiError {
    ApiError::Invalid {
        field: field.into(),
    }
}

impl Engine {
    pub(crate) fn endpoints_view(&self) -> EndpointsView {
        let e = self.current_endpoints();
        EndpointsView {
            profiles: e
                .profiles
                .iter()
                .map(|p| ProfileView {
                    profile: p.clone(),
                    has_key: self.ports.keys.has(&p.name),
                })
                .collect(),
            roles: e.roles,
            presets: presets(),
        }
    }

    /// Check, write the file in one rename, then tell everything that reads it.
    fn save_endpoints(&self, next: crate::endpoints::Endpoints) -> ApiResult<()> {
        next.validate().map_err(|_| invalid("profile"))?;
        let text = next.to_toml().map_err(io)?;
        let path = self.home.endpoints();
        let tmp = path.with_extension("toml.part");
        std::fs::write(&tmp, text).map_err(io)?;
        std::fs::rename(&tmp, &path).map_err(io)?;
        self.ports.agent.set_endpoints(&next);
        *self.endpoints.lock().unwrap_or_else(|e| e.into_inner()) = next;
        Ok(())
    }

    fn check(profile: &Profile) -> ApiResult<()> {
        if profile.name.trim().is_empty() {
            return Err(invalid("name"));
        }
        if profile.base_url.trim().is_empty() {
            return Err(invalid("base_url"));
        }
        Ok(())
    }

    pub(crate) fn add_profile_impl(&self, profile: Profile) -> ApiResult<()> {
        Self::check(&profile)?;
        let mut e = self.current_endpoints();
        if e.profile(&profile.name).is_some() {
            return Err(invalid("name"));
        }
        e.profiles.push(profile);
        self.save_endpoints(e)
    }

    pub(crate) fn update_profile_impl(&self, name: &str, profile: Profile) -> ApiResult<()> {
        Self::check(&profile)?;
        let mut e = self.current_endpoints();
        let at = e
            .profiles
            .iter()
            .position(|p| p.name == name)
            .ok_or(ApiError::NotFound)?;
        let renamed = profile.name != name;
        if renamed && e.profile(&profile.name).is_some() {
            return Err(invalid("name"));
        }
        if renamed {
            for role in e.roles.iter_mut().flat_map(|r| {
                [Some(&mut r.chat), r.tts.as_mut(), r.persona.as_mut()]
                    .into_iter()
                    .flatten()
            }) {
                if role.profile == name {
                    role.profile = profile.name.clone();
                }
            }
            self.ports.keys.rename(name, &profile.name)?;
        }
        e.profiles[at] = profile;
        self.save_endpoints(e)
    }

    pub(crate) fn remove_profile_impl(&self, name: &str) -> ApiResult<()> {
        let mut e = self.current_endpoints();
        if e.profile(name).is_none() {
            return Err(ApiError::NotFound);
        }
        let used = e.roles.iter().any(|r| {
            [Some(&r.chat), r.tts.as_ref(), r.persona.as_ref()]
                .into_iter()
                .flatten()
                .any(|role| role.profile == name)
        });
        if used {
            return Err(invalid("profile"));
        }
        e.profiles.retain(|p| p.name != name);
        self.save_endpoints(e)?;
        self.ports.keys.delete(name)
    }

    pub(crate) async fn fetch_models_impl(&self, name: &str) -> ApiResult<Vec<String>> {
        let profile = self
            .current_endpoints()
            .profile(name)
            .cloned()
            .ok_or(ApiError::NotFound)?;
        self.ports.llm.models(&profile).await
    }

    pub(crate) fn set_key_impl(&self, profile: &str, key: &str) -> ApiResult<()> {
        if self.current_endpoints().profile(profile).is_none() {
            return Err(ApiError::NotFound);
        }
        let key = key.trim();
        if key.is_empty() {
            return Err(invalid("key"));
        }
        self.ports.keys.set(profile, key)
    }

    pub(crate) fn set_role_impl(&self, kind: RoleKind, role: Option<Role>) -> ApiResult<()> {
        let mut e = self.current_endpoints();
        if let Some(r) = &role
            && e.profile(&r.profile).is_none()
        {
            return Err(invalid("profile"));
        }
        match (kind, role) {
            (RoleKind::Chat, Some(chat)) => match e.roles.as_mut() {
                Some(r) => r.chat = chat,
                None => {
                    e.roles = Some(Roles {
                        chat,
                        tts: None,
                        persona: None,
                    })
                }
            },
            // Only read-aloud can be switched off; the chat role is replaced, not removed.
            (RoleKind::Chat, None) => return Err(invalid("role")),
            (RoleKind::Tts, tts) => match e.roles.as_mut() {
                Some(r) => r.tts = tts,
                None if tts.is_none() => {}
                None => return Err(invalid("chat")),
            },
        }
        self.save_endpoints(e)
    }
}
