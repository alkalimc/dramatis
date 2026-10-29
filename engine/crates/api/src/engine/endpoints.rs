//! Endpoint profiles and roles: the file, the keychain and the endpoint-side lookups.

use super::{Engine, io};
use crate::endpoints::{Model, Profile, Role, Roles, presets};
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
        let url = profile.base_url.trim();
        if !(url.starts_with("http://") || url.starts_with("https://")) || url.contains(' ') {
            return Err(invalid("base_url"));
        }
        Ok(())
    }

    pub(crate) fn setup_endpoint_impl(&self, setup: EndpointSetup) -> ApiResult<()> {
        let id = |text: &str, field: &str| {
            let t = text.trim();
            if t.is_empty() || t.chars().any(char::is_whitespace) {
                Err(invalid(field))
            } else {
                Ok(t.to_owned())
            }
        };
        let model = id(&setup.model, "model")?;
        let host_model = setup
            .host_model
            .as_deref()
            .filter(|m| !m.trim().is_empty())
            .map(|m| id(m, "host_model"))
            .transpose()?;
        let name = setup.name.trim().to_owned();
        let key = setup
            .key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty());
        let mut e = self.current_endpoints();
        let mut profile = e.profile(&name).cloned().unwrap_or(Profile {
            name: name.clone(),
            base_url: String::new(),
            wire_api: setup.wire_api,
            models: Vec::new(),
        });
        profile.base_url = setup.base_url.trim().to_owned();
        profile.wire_api = setup.wire_api;
        for m in [Some(&model), host_model.as_ref()].into_iter().flatten() {
            if !profile.models.iter().any(|known| &known.id == m) {
                profile.models.push(Model {
                    id: m.clone(),
                    ..Model::default()
                });
            }
        }
        Self::check(&profile)?;
        let role = |m: &String| Role {
            profile: name.clone(),
            model: m.clone(),
            reasoning: None,
        };
        match e.profiles.iter().position(|p| p.name == name) {
            Some(at) => e.profiles[at] = profile,
            None => e.profiles.push(profile),
        }
        let chat = role(&model);
        let host = host_model.as_ref().map(role);
        match e.roles.as_mut() {
            Some(r) => {
                r.chat = chat;
                r.host = host;
            }
            None => {
                e.roles = Some(Roles {
                    chat,
                    host,
                    tts: None,
                    persona: None,
                })
            }
        }
        // The key first: when the keychain refuses, nothing else has changed.
        if let Some(key) = key {
            self.ports.keys.set(&name, key)?;
        }
        self.save_endpoints(e)
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
                [
                    Some(&mut r.chat),
                    r.host.as_mut(),
                    r.tts.as_mut(),
                    r.persona.as_mut(),
                ]
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
            [
                Some(&r.chat),
                r.host.as_ref(),
                r.tts.as_ref(),
                r.persona.as_ref(),
            ]
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
                        host: None,
                        tts: None,
                        persona: None,
                    })
                }
            },
            (RoleKind::Host, host) => match e.roles.as_mut() {
                Some(r) => r.host = host,
                None if host.is_none() => {}
                None => return Err(invalid("chat")),
            },
            // Only read-aloud and the host can be unset; the chat role is replaced, not removed.
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
