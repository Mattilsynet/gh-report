from dataclasses import dataclass
import re


@dataclass(frozen=True)
class Unknown:
    reason: str


@dataclass(frozen=True)
class System:
    reason: str


@dataclass(frozen=True)
class LinkedUser:
    login: str


@dataclass(frozen=True)
class Selected:
    role: str
    actor: LinkedUser


def classify(raw):
    if not isinstance(raw, dict):
        return Unknown("missing or malformed GitHub user")
    login = raw.get("login")
    if raw.get("type") == "Bot":
        return System("GitHub Bot type")
    if isinstance(login, str) and (login.lower() == "web-flow" or login.lower().endswith("[bot]")):
        return System("system login")
    if not isinstance(login, str) or re.fullmatch(r"[A-Za-z0-9]+(?:-[A-Za-z0-9]+)*", login) is None:
        return Unknown("unusable login")
    if raw.get("type") != "User":
        return Unknown("unobserved or unsupported account type")
    return LinkedUser(login)


def select(commit):
    if not isinstance(commit, dict):
        return Unknown("commit unavailable")
    for role in ("committer", "author"):
        actor = classify(commit.get(role))
        if isinstance(actor, LinkedUser):
            return Selected(role, actor)
    return Unknown("no eligible linked candidate")
