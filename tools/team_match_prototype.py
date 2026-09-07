from dataclasses import dataclass
from enum import Enum


class Coverage(Enum):
    PARTIAL_VISIBILITY = "partial_visibility"
    INCOMPLETE = "incomplete"
    UNAUTHORIZED = "unauthorized"
    COMPLETE = "complete"


@dataclass(frozen=True)
class TeamDto:
    slug: str
    display_name: str


@dataclass(frozen=True)
class Provenance:
    organization: str
    observation: str
    coverage: Coverage


@dataclass(frozen=True)
class Limits:
    max_teams: int
    max_field_chars: int

    def __post_init__(self):
        if self.max_teams < 0 or self.max_field_chars < 1:
            raise ValueError("invalid prototype limits")


@dataclass(frozen=True)
class Exact:
    raw_owner: str
    team: TeamDto
    provenance: Provenance


@dataclass(frozen=True)
class CaseDiscrepancy:
    raw_owner: str
    team: TeamDto
    provenance: Provenance


class UnknownReason(Enum):
    INVALID_LIMITS = "invalid_limits"
    INVALID_PROVENANCE = "invalid_provenance"
    UNAUTHORIZED = "unauthorized"
    CATALOG_BOUND_OR_SHAPE = "catalog_bound_or_shape"
    INVALID_OWNER = "invalid_owner"
    DIFFERENT_ORGANIZATION = "different_organization"
    INVALID_CATALOG_ENTRY = "invalid_catalog_entry"
    AMBIGUOUS_IDENTITY = "ambiguous_identity"
    NOT_OBSERVED = "not_observed"


@dataclass(frozen=True)
class Unknown:
    reason: UnknownReason

    def __post_init__(self):
        if not isinstance(self.reason, UnknownReason):
            raise TypeError("reason must be an UnknownReason")


def match_owner(raw_owner, teams, provenance, limits):
    def bounded_text(value):
        return isinstance(value, str) and 0 < len(value) <= limits.max_field_chars

    def identifier(value):
        return bounded_text(value) and value.isascii() and all(
            char.isalnum() or char == "-" for char in value
        )

    if not isinstance(limits, Limits):
        return Unknown(UnknownReason.INVALID_LIMITS)
    if not isinstance(provenance, Provenance):
        return Unknown(UnknownReason.INVALID_PROVENANCE)
    if (not identifier(provenance.organization)
            or not bounded_text(provenance.observation)
            or not isinstance(provenance.coverage, Coverage)):
        return Unknown(UnknownReason.INVALID_PROVENANCE)
    if provenance.coverage is Coverage.UNAUTHORIZED:
        return Unknown(UnknownReason.UNAUTHORIZED)
    if type(teams) is not tuple or len(teams) > limits.max_teams:
        return Unknown(UnknownReason.CATALOG_BOUND_OR_SHAPE)
    if not bounded_text(raw_owner) or not raw_owner.startswith("@"):
        return Unknown(UnknownReason.INVALID_OWNER)
    parts = raw_owner[1:].split("/")
    if len(parts) != 2 or not all(identifier(part) for part in parts):
        return Unknown(UnknownReason.INVALID_OWNER)
    organization, slug = parts
    if organization.lower() != provenance.organization.lower():
        return Unknown(UnknownReason.DIFFERENT_ORGANIZATION)
    candidate = None
    for team in teams:
        if (not isinstance(team, TeamDto) or not identifier(team.slug)
                or not bounded_text(team.display_name)):
            return Unknown(UnknownReason.INVALID_CATALOG_ENTRY)
        if team.slug.lower() == slug.lower():
            if candidate is not None and candidate != team:
                return Unknown(UnknownReason.AMBIGUOUS_IDENTITY)
            candidate = team
    if candidate is None:
        return Unknown(UnknownReason.NOT_OBSERVED)
    if candidate.slug == slug:
        return Exact(raw_owner, candidate, provenance)
    return CaseDiscrepancy(raw_owner, candidate, provenance)
