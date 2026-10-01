//! Endpoint access: which model a role points at, where its key comes from, and the
//! transport that sends a prepared body and returns its event stream.
//!
//! Bodies are built byte for byte from the session log ([`crate::wire`]), so the HTTP
//! client only carries them: `async-openai`'s bring-your-own-types calls post a raw JSON
//! value and parse the SSE stream, and nothing re-serialises the request on the way.

use std::fmt;
use std::sync::Arc;

use api::endpoints::{Endpoints, KEYCHAIN_SERVICE, Model, Profile, WireApi, keychain_account};
use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::Value;
use serde_json::value::RawValue;
use world::quota::Prices;

use crate::error::{Error, Result};

/// A key held only as long as a request needs it. Never printed.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// Where a call goes.
#[derive(Debug, Clone)]
pub struct Target {
    pub base_url: String,
    pub wire: WireApi,
    /// `None` for an endpoint that takes no key (a local server).
    pub key: Option<Secret>,
}

/// The chat role, resolved: profile, model and what is known about the model.
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
        let role = endpoints
            .roles
            .as_ref()
            .ok_or(Error::NoEndpoint)?
            .chat
            .clone();
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
}

/// Where profile keys are read from.
pub trait Keys: Send + Sync {
    fn get(&self, profile: &str) -> Result<Option<Secret>>;
}

/// The system keychain: service `dramatis`, account `profile.<name>`.
#[derive(Debug, Default, Clone, Copy)]
pub struct Keychain;

fn entry(profile: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYCHAIN_SERVICE, &keychain_account(profile))
        .map_err(|e| Error::Keychain(e.to_string()))
}

impl Keychain {
    /// Store a profile's key. It is never read back out of this crate except into a
    /// request header.
    pub fn set(profile: &str, key: &str) -> Result<()> {
        entry(profile)?
            .set_password(key)
            .map_err(|e| Error::Keychain(e.to_string()))
    }

    pub fn delete(profile: &str) -> Result<()> {
        match entry(profile)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(Error::Keychain(e.to_string())),
        }
    }
}

impl Keys for Keychain {
    fn get(&self, profile: &str) -> Result<Option<Secret>> {
        match entry(profile)?.get_password() {
            Ok(k) => Ok(Some(Secret(k))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Error::Keychain(e.to_string())),
        }
    }
}

/// Keys held in memory: tests, and endpoints without a key.
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

pub type Events = BoxStream<'static, Result<Value>>;

/// Sends one prepared body and yields the decoded SSE `data` payloads.
#[async_trait]
pub trait Transport: Send + Sync {
    async fn stream(&self, target: &Target, body: Vec<u8>) -> Result<Events>;
    /// `GET {base_url}/models`: model ids.
    async fn models(&self, target: &Target) -> Result<Vec<String>>;
}

/// HTTP through `async-openai`. The client of the last target is kept, so consecutive
/// calls reuse its connection (TLS setup is a visible share of the first token's wait).
#[derive(Default)]
pub struct Http {
    last: std::sync::Mutex<Option<(String, String, Client<OpenAIConfig>)>>,
}

impl fmt::Debug for Http {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Http")
    }
}

impl Http {
    fn client(&self, target: &Target) -> Client<OpenAIConfig> {
        let base = target.base_url.trim_end_matches('/').to_owned();
        let key = target.key.as_ref().map(Secret::expose).unwrap_or("");
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((b, k, c)) = last.as_ref()
            && *b == base
            && k == key
        {
            return c.clone();
        }
        // Organisation and project headers would come from the environment otherwise:
        // they mean nothing to other providers. Without a key an empty one is sent,
        // which local servers ignore.
        let config = OpenAIConfig::new()
            .with_api_base(base.clone())
            .with_api_key(key)
            .with_org_id("")
            .with_project_id("");
        let c = Client::with_config(config);
        *last = Some((base, key.to_owned(), c.clone()));
        c
    }
}

fn endpoint(e: async_openai::error::OpenAIError) -> Error {
    Error::Endpoint {
        detail: e.to_string(),
    }
}

#[async_trait]
impl Transport for Http {
    async fn stream(&self, target: &Target, body: Vec<u8>) -> Result<Events> {
        let raw: Box<RawValue> = serde_json::from_slice(&body)?;
        let c = self.client(target);
        let events = match target.wire {
            WireApi::Chat => c.chat().create_stream_byot::<_, Value>(raw).await,
            WireApi::Responses => c.responses().create_stream_byot::<_, Value>(raw).await,
        }
        .map_err(endpoint)?;
        Ok(events.map(|e| e.map_err(endpoint)).boxed())
    }

    async fn models(&self, target: &Target) -> Result<Vec<String>> {
        let list: Value = self
            .client(target)
            .models()
            .list_byot()
            .await
            .map_err(endpoint)?;
        Ok(list["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m["id"].as_str().map(str::to_owned))
            .collect())
    }
}

/// The target of a profile, with its key from `keys`.
pub fn target(profile: &Profile, keys: &dyn Keys) -> Result<Target> {
    Ok(Target {
        base_url: profile.base_url.clone(),
        wire: profile.wire_api,
        key: keys.get(&profile.name)?,
    })
}

pub type SharedTransport = Arc<dyn Transport>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_do_not_print_and_roles_resolve() {
        assert_eq!(format!("{:?}", Secret::new("sk-123")), "Secret(..)");
        let e = Endpoints::from_toml(
            "[[profile]]\nname='p'\nbase_url='http://127.0.0.1:1/v1'\nwire_api='responses'\n\
             [[profile.model]]\nid='m'\nforced_tool=true\nprice_in=1.0\nprice_in_cached=0.1\nprice_out=4.0\n\
             [roles]\nchat={profile='p', model='m'}\n",
        )
        .unwrap();
        let r = ChatRole::resolve(&e).unwrap();
        assert!(r.forced_tool());
        assert_eq!(r.prices().unwrap().cached_input, 0.1);
        let unlisted = Endpoints::from_toml(
            "[[profile]]\nname='p'\nbase_url='x'\nwire_api='chat'\n[roles]\nchat={profile='p', model='typed'}\n",
        )
        .unwrap();
        let r = ChatRole::resolve(&unlisted).unwrap();
        assert!(!r.forced_tool());
        assert_eq!(r.model.id, "typed");
        assert!(matches!(
            ChatRole::resolve(&Endpoints::default()),
            Err(Error::NoEndpoint)
        ));
        let keys = StaticKeys(vec![("p".into(), Secret::new("k"))]);
        let t = target(&unlisted.profiles[0], &keys).unwrap();
        assert!(format!("{t:?}").contains("Secret(..)"));
    }
}
