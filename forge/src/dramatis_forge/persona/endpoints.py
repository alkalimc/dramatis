"""`$DRAMATIS_HOME/endpoints.toml`, read the same way the engine reads it.

The engine's serde types are the contract; these dataclasses mirror them field for field
(unknown keys rejected, duplicate profiles rejected, roles must name a defined profile).
The forge needs only `roles.persona` and the profile it names.

The file holds no secret. A profile's key lives only in the system keychain, service
`dramatis`, account `profile.<name>`, and is never read from the environment or a file.
"""

from __future__ import annotations

import os
import tomllib
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

KEYCHAIN_SERVICE = "dramatis"
ENV_HOME = "DRAMATIS_HOME"
WIRE_APIS = ("chat", "responses")


class EndpointsError(Exception):
    """The file is missing, malformed, or does not define what `forge persona` needs.
    `hint` says how to fix it."""

    def __init__(self, message: str, hint: str = "") -> None:
        super().__init__(message)
        self.hint = hint


def keychain_account(profile: str) -> str:
    return f"profile.{profile}"


def dramatis_home() -> Path:
    return Path(os.environ.get(ENV_HOME) or "~/.dramatis").expanduser()


def default_path() -> Path:
    return dramatis_home() / "endpoints.toml"


@dataclass(frozen=True)
class Model:
    id: str
    forced_tool: bool | None = None
    context_window: int | None = None
    cache_min_prefix: int | None = None
    cache_breakpoints: int | None = None
    price_in: float | None = None
    price_in_cached: float | None = None
    price_out: float | None = None


@dataclass(frozen=True)
class Profile:
    name: str
    base_url: str
    wire_api: str
    models: tuple[Model, ...] = ()


@dataclass(frozen=True)
class Role:
    profile: str
    model: str
    #: The endpoint's reasoning parameter; None sends none.
    reasoning: str | None = None


@dataclass(frozen=True)
class Roles:
    chat: Role
    tts: Role | None = None
    persona: Role | None = None


@dataclass(frozen=True)
class Endpoints:
    profiles: tuple[Profile, ...] = ()
    roles: Roles | None = None

    def profile(self, name: str) -> Profile | None:
        return next((p for p in self.profiles if p.name == name), None)

    @classmethod
    def from_toml(cls, text: str) -> Endpoints:
        try:
            raw = tomllib.loads(text)
        except tomllib.TOMLDecodeError as exc:
            raise EndpointsError(f"endpoints file: {exc}") from exc
        _only(raw, {"profile", "roles"}, "the file")
        profiles = tuple(_profile(p) for p in raw.get("profile", []))
        seen: set[str] = set()
        for p in profiles:
            if p.name in seen:
                raise EndpointsError(f"profile `{p.name}` is defined twice")
            seen.add(p.name)
        roles = None
        if "roles" in raw:
            r = raw["roles"]
            _only(r, {"chat", "tts", "persona"}, "[roles]")
            if "chat" not in r:
                raise EndpointsError("[roles] has no `chat` role")
            roles = Roles(chat=_role(r["chat"], "chat"),
                          tts=_role(r["tts"], "tts") if "tts" in r else None,
                          persona=_role(r["persona"], "persona") if "persona" in r else None)
            for name, role in (("chat", roles.chat), ("tts", roles.tts),
                               ("persona", roles.persona)):
                if role is not None and role.profile not in seen:
                    raise EndpointsError(
                        f"role `{name}` names profile `{role.profile}`, which is not defined")
        return cls(profiles=profiles, roles=roles)

    @classmethod
    def load(cls, path: Path | None = None) -> Endpoints:
        path = path or default_path()
        if not path.exists():
            raise EndpointsError(
                f"no endpoints file at {path}",
                "copy the maintainer's endpoints.toml there, or pass --endpoints PATH")
        return cls.from_toml(path.read_text(encoding="utf-8"))


def _only(table: Any, allowed: set[str], where: str) -> None:
    if not isinstance(table, dict):
        raise EndpointsError(f"endpoints file: {where} must be a table")
    unknown = sorted(set(table) - allowed)
    if unknown:
        # A key in the file must never be silently accepted and ignored (an `api_key`
        # line, most of all).
        raise EndpointsError(f"endpoints file: unknown key(s) {unknown} in {where}")


def _need(table: dict, key: str, kind: type, where: str) -> Any:
    value = table.get(key)
    if not isinstance(value, kind) or isinstance(value, bool) and kind is not bool:
        raise EndpointsError(f"endpoints file: {where} needs `{key}` ({kind.__name__})")
    return value


def _opt(table: dict, key: str, kind: type | tuple[type, ...], where: str) -> Any:
    value = table.get(key)
    if value is not None and (not isinstance(value, kind)
                              or isinstance(value, bool) and kind is not bool):
        raise EndpointsError(f"endpoints file: `{key}` in {where} has the wrong type")
    return value


def _profile(raw: Any) -> Profile:
    _only(raw, {"name", "base_url", "wire_api", "model"}, "[[profile]]")
    name = _need(raw, "name", str, "[[profile]]")
    where = f"profile `{name}`"
    wire = _need(raw, "wire_api", str, where)
    if wire not in WIRE_APIS:
        raise EndpointsError(f"endpoints file: {where} has wire_api `{wire}`; "
                             f"expected one of {', '.join(WIRE_APIS)}")
    models = []
    for m in raw.get("model", []):
        _only(m, {f.name for f in Model.__dataclass_fields__.values()}, f"{where} model")
        models.append(Model(
            id=_need(m, "id", str, f"{where} model"),
            forced_tool=_opt(m, "forced_tool", bool, where),
            context_window=_opt(m, "context_window", int, where),
            cache_min_prefix=_opt(m, "cache_min_prefix", int, where),
            cache_breakpoints=_opt(m, "cache_breakpoints", int, where),
            price_in=_opt(m, "price_in", (int, float), where),
            price_in_cached=_opt(m, "price_in_cached", (int, float), where),
            price_out=_opt(m, "price_out", (int, float), where),
        ))
    return Profile(name=name, base_url=_need(raw, "base_url", str, where), wire_api=wire,
                   models=tuple(models))


def _role(raw: Any, name: str) -> Role:
    where = f"role `{name}`"
    _only(raw, {"profile", "model", "reasoning"}, where)
    return Role(profile=_need(raw, "profile", str, where), model=_need(raw, "model", str, where),
                reasoning=_opt(raw, "reasoning", str, where))


@dataclass(frozen=True)
class Target:
    """Everything one call needs: where, which model, how hard to think, and the key."""

    profile: Profile
    role: Role
    key: str = field(repr=False)


KeyLookup = Callable[[str, str], str | None]


def _keyring_lookup(service: str, account: str) -> str | None:
    import keyring

    return keyring.get_password(service, account)


def persona_target(endpoints: Endpoints, lookup: KeyLookup | None = None) -> Target:
    """Resolve `roles.persona` to a profile and its key. `lookup(service, account)` is the
    keychain read; injected by tests, `keyring.get_password` otherwise."""
    role = endpoints.roles.persona if endpoints.roles else None
    if role is None:
        raise EndpointsError(
            "the endpoints file defines no `persona` role",
            'add under [roles]: persona = { profile = "<name>", model = "<id>", '
            'reasoning = "high" }')
    profile = endpoints.profile(role.profile)
    assert profile is not None  # checked when parsing
    account = keychain_account(profile.name)
    fix = f"security add-generic-password -s {KEYCHAIN_SERVICE} -a {account} -w"
    try:
        key = (lookup or _keyring_lookup)(KEYCHAIN_SERVICE, account)
    except Exception as exc:  # no keychain backend, locked keychain, denied access
        raise EndpointsError(f"could not read the keychain entry {KEYCHAIN_SERVICE}/{account}: "
                             f"{exc}", f"store the key with: {fix}") from exc
    if not key:
        raise EndpointsError(f"no key in the keychain for {KEYCHAIN_SERVICE}/{account}",
                             f"store it with: {fix}")
    return Target(profile=profile, role=role, key=key)
