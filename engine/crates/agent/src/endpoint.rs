//! Endpoint access: which model a role points at, where its key comes from, and the
//! [`llm::Endpoint`] a call goes to.

use api::endpoints::{Endpoints, Model, Profile, WireApi};
use llm::{Caps, Endpoint, Protocol, Secret};
use world::quota::Prices;

use crate::error::{Error, Result};

/// A runtime role (chat or host), resolved: profile, model and what is known about the model.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatRole {
    pub profile: Profile,
    pub model: Model,
    pub reasoning: Option<String>,
}

impl ChatRole {
    /// The chat role of an endpoints file. A model the profile does not list is known
    /// only by name: no forced tool (wrap-up skipped), no window, default prices.
    pub fn resolve(endpoints: &Endpoints) -> Result<Self> {
        Self::from_role(endpoints, &Self::roles(endpoints)?.chat)
    }

    /// The host role; the chat role where the file has none.
    pub fn resolve_host(endpoints: &Endpoints) -> Result<Self> {
        let roles = Self::roles(endpoints)?;
        Self::from_role(endpoints, roles.host.as_ref().unwrap_or(&roles.chat))
    }

    fn roles(endpoints: &Endpoints) -> Result<&api::endpoints::Roles> {
        endpoints.roles.as_ref().ok_or(Error::NoEndpoint)
    }

    fn from_role(endpoints: &Endpoints, role: &api::endpoints::Role) -> Result<Self> {
        let role = role.clone();
        let profile = endpoints
            .profile(&role.profile)
            .ok_or(Error::NoEndpoint)?
            .clone();
        let model = profile
            .models
            .iter()
            .find(|m| m.id == role.model)
            .cloned()
            .unwrap_or_else(|| Model {
                id: role.model.clone(),
                ..Model::default()
            });
        Ok(Self {
            profile,
            model,
            reasoning: role.reasoning,
        })
    }

    pub fn forced_tool(&self) -> bool {
        self.model.forced_tool == Some(true)
    }

    pub fn prices(&self) -> Option<Prices> {
        match (
            self.model.price_in,
            self.model.price_in_cached,
            self.model.price_out,
        ) {
            (Some(input), Some(cached_input), Some(output)) => Some(Prices {
                input,
                cached_input,
                output,
            }),
            _ => None,
        }
    }

    /// Price of writing a cache entry relative to the plain input price, where both
    /// are configured.
    pub fn cache_write_ratio(&self) -> Option<f64> {
        match (self.model.price_in, self.model.price_in_cache_write) {
            (Some(input), Some(write)) if input > 0.0 && write >= 0.0 => Some(write / input),
            _ => None,
        }
    }
}

/// Where profile keys come from when they are not in the system keychain: tests, and
/// endpoints without a key.
pub trait Keys: Send + Sync {
    fn get(&self, profile: &str) -> Result<Option<Secret>>;
}

/// Keys held in memory.
#[derive(Debug, Default, Clone)]
pub struct StaticKeys(pub Vec<(String, Secret)>);

impl Keys for StaticKeys {
    fn get(&self, profile: &str) -> Result<Option<Secret>> {
        Ok(self
            .0
            .iter()
            .find(|(p, _)| p == profile)
            .map(|(_, k)| k.clone()))
    }
}

pub fn protocol(wire: WireApi) -> Protocol {
    match wire {
        WireApi::Chat => Protocol::OpenAiChat,
        WireApi::Responses => Protocol::OpenAiResponses,
        WireApi::Messages => Protocol::AnthropicMessages,
    }
}

/// The endpoint of a profile and model. `key` is the profile's, if it has one.
pub fn endpoint(profile: &Profile, model: &Model, key: Option<Secret>) -> Endpoint {
    Endpoint {
        protocol: protocol(profile.wire_api),
        base_url: profile.base_url.clone(),
        model: model.id.clone(),
        key: key.unwrap_or_default(),
        caps: Caps {
            forced_tool: model.forced_tool == Some(true),
            cache_breakpoints: model.cache_breakpoints,
            context_window: model.context_window,
            cache_min_prefix: model.cache_min_prefix,
        },
    }
}

/// The profile's key from `keys`, or from the system keychain when there are none.
pub fn key(keys: Option<&dyn Keys>, profile: &str) -> Result<Option<Secret>> {
    match keys {
        Some(k) => k.get(profile),
        None => Ok(llm::keychain::get(profile)?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_resolve_and_endpoints_carry_what_is_known() {
        let e = Endpoints::from_toml(
            "[[profile]]\nname='p'\nbase_url='http://127.0.0.1:1/v1'\nwire_api='messages'\n\
             [[profile.model]]\nid='m'\nforced_tool=true\ncontext_window=1000\ncache_min_prefix=512\n\
             cache_breakpoints=2\nprice_in=1.0\nprice_in_cached=0.1\nprice_in_cache_write=1.5\nprice_out=4.0\n\
             [roles]\nchat={profile='p', model='m'}\n",
        )
        .unwrap();
        let r = ChatRole::resolve(&e).unwrap();
        assert!(r.forced_tool());
        assert_eq!(r.prices().unwrap().cached_input, 0.1);
        assert_eq!(r.cache_write_ratio(), Some(1.5));
        let ep = endpoint(&r.profile, &r.model, Some(Secret::new("sk-123")));
        assert_eq!(ep.protocol, Protocol::AnthropicMessages);
        assert_eq!(ep.model, "m");
        assert_eq!(
            ep.caps,
            Caps {
                forced_tool: true,
                cache_breakpoints: Some(2),
                context_window: Some(1000),
                cache_min_prefix: Some(512),
            }
        );
        assert!(!format!("{ep:?}").contains("sk-123"));
        let unlisted = Endpoints::from_toml(
            "[[profile]]\nname='p'\nbase_url='x'\nwire_api='chat'\n[roles]\nchat={profile='p', model='typed'}\n",
        )
        .unwrap();
        let r = ChatRole::resolve(&unlisted).unwrap();
        assert!(!r.forced_tool());
        assert_eq!(r.model.id, "typed");
        assert_eq!(r.cache_write_ratio(), None);
        assert!(matches!(
            ChatRole::resolve(&Endpoints::default()),
            Err(Error::NoEndpoint)
        ));
        let with_host = Endpoints::from_toml(
            "[[profile]]\nname='p'\nbase_url='x'\nwire_api='chat'\n[roles]\n\
             chat={profile='p', model='c'}\nhost={profile='p', model='h', reasoning='low'}\n",
        )
        .unwrap();
        let h = ChatRole::resolve_host(&with_host).unwrap();
        assert_eq!(
            (h.model.id.as_str(), h.reasoning.as_deref()),
            ("h", Some("low"))
        );
        assert_eq!(ChatRole::resolve(&with_host).unwrap().model.id, "c");
        assert_eq!(ChatRole::resolve_host(&unlisted).unwrap().model.id, "typed");
        let keys = StaticKeys(vec![("p".into(), Secret::new("k"))]);
        assert!(key(Some(&keys), "p").unwrap().is_some());
        assert!(key(Some(&keys), "q").unwrap().is_none());
    }
}
