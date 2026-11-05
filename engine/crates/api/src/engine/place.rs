//! Where the app's files live, how its connections leave the machine, and the endpoint
//! test: the settings that sit beside the endpoints rather than inside a save.

use super::{Engine, io};
use crate::error::{ApiError, ApiResult};
use crate::home::{self, Source};
use crate::port::PROXY_ACCOUNT;
use crate::relocate;
use crate::views::*;

fn invalid(field: &str) -> ApiError {
    ApiError::Invalid {
        field: field.into(),
    }
}

/// The setting as it is kept (trimmed), or the field that is wrong. A proxy address is an
/// http, https, socks5 or socks5h scheme and a host (and port), with nothing after it and
/// no login in it: the login has its own fields, so the address file never holds one.
pub(crate) fn check_network(setting: &NetworkSetting) -> ApiResult<NetworkSetting> {
    let proxy = setting.proxy.trim();
    let username = setting.username.trim();
    let no_proxy = setting.no_proxy.trim();
    if username.chars().any(char::is_control) || username.contains(':') {
        return Err(invalid("username"));
    }
    if no_proxy.chars().any(char::is_control) || no_proxy.len() > 2000 {
        return Err(invalid("no_proxy"));
    }
    if setting.mode == NetworkMode::Manual || !proxy.is_empty() {
        check_proxy(proxy)?;
    }
    Ok(NetworkSetting {
        mode: setting.mode,
        proxy: proxy.to_owned(),
        username: username.to_owned(),
        no_proxy: no_proxy.to_owned(),
    })
}

fn check_proxy(proxy: &str) -> ApiResult<()> {
    let bad = || invalid("proxy");
    let (scheme, rest) = proxy.split_once("://").ok_or_else(bad)?;
    if !["http", "https", "socks5", "socks5h"].contains(&scheme.to_ascii_lowercase().as_str()) {
        return Err(bad());
    }
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    if authority.is_empty()
        || authority.contains(['/', '?', '#', '@'])
        || authority
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(bad());
    }
    // `host`, `host:port`, `[v6]` or `[v6]:port`.
    let (host, port) = match authority.strip_prefix('[') {
        Some(v6) => {
            let (host, tail) = v6.split_once(']').ok_or_else(bad)?;
            (
                host,
                tail.strip_prefix(':').or(tail.is_empty().then_some("")),
            )
        }
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, Some("")),
        },
    };
    let port = port.ok_or_else(bad)?;
    if host.is_empty() || (!port.is_empty() && port.parse::<u16>().is_err()) {
        return Err(bad());
    }
    Ok(())
}

impl Engine {
    pub(crate) fn home_location_impl(&self) -> HomeLocation {
        HomeLocation {
            path: self.home.root().display().to_string(),
            default_path: home::default_root().display().to_string(),
            source: match self.home.source() {
                Source::Default => HomeSource::Default,
                Source::Chosen => HomeSource::Chosen,
                Source::Override => HomeSource::Fixed,
            },
            unavailable: self.home.unavailable().map(|p| p.display().to_string()),
            can_change: self.home.can_relocate(),
        }
    }

    pub(crate) fn set_home_location_impl(
        &self,
        path: Option<String>,
        data: HomeData,
    ) -> ApiResult<()> {
        if !self.home.can_relocate() {
            return Err(invalid("home"));
        }
        // A copy is a picture of the saves at one moment; nothing may be writing meanwhile.
        if !self.ports.agent.idle() {
            return Err(ApiError::Busy);
        }
        let fresh = match &path {
            Some(typed) => relocate::check_target(&self.home, typed)?,
            None => home::default_root(),
        };
        let same = |a: &std::path::Path, b: &std::path::Path| {
            a == b
                || matches!(
                    (std::fs::canonicalize(a), std::fs::canonicalize(b)),
                    (Ok(a), Ok(b)) if a == b
                )
        };
        if same(&fresh, self.home.root()) {
            return Err(invalid("path"));
        }
        if data == HomeData::Copy {
            std::fs::create_dir_all(&fresh).map_err(io)?;
            relocate::copy_home(&self.home, &fresh, &|name| self.password_of(name))?;
        }
        // The pointer last: until it is written, the next start is where this one is.
        let back = path.is_none();
        self.home
            .choose_root(if back { None } else { Some(&fresh) })
            .map_err(io)?;
        (self.ports.restart)();
        Ok(())
    }

    pub(crate) fn network_view(&self) -> NetworkView {
        NetworkView {
            setting: self.home.network(),
            has_password: self.ports.keys.has(PROXY_ACCOUNT),
        }
    }

    pub(crate) fn set_network_impl(
        &self,
        setting: NetworkSetting,
        password: Option<String>,
    ) -> ApiResult<()> {
        let setting = check_network(&setting)?;
        // The keychain first: when it refuses, nothing else has changed.
        match password.as_deref() {
            Some("") => self.ports.keys.delete(PROXY_ACCOUNT)?,
            Some(p) => self.ports.keys.set(PROXY_ACCOUNT, p)?,
            None => {}
        }
        self.ports
            .net
            .set_network(&setting)
            .map_err(|_| invalid("proxy"))?;
        self.home.set_network(&setting).map_err(io)
    }

    pub(crate) async fn test_endpoint_impl(
        &self,
        name: &str,
        model: &str,
    ) -> ApiResult<EndpointTest> {
        let profile = self
            .current_endpoints()
            .profile(name)
            .cloned()
            .ok_or(ApiError::NotFound)?;
        let model = model.trim();
        if model.is_empty() || model.chars().any(char::is_whitespace) {
            return Err(invalid("model"));
        }
        self.ports.llm.probe(&profile, model).await
    }
}
