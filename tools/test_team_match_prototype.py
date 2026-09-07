import unittest
from dataclasses import FrozenInstanceError, replace
from datetime import timedelta
from html import escape

from team_match_prototype import (
    CaseDiscrepancy, Coverage, Exact, Limits, Provenance, TeamDto, Unknown,
    UnknownReason, match_owner,
)
from observation_prototype import Instant, ProgressAge, Running, observe


class MatchingTests(unittest.TestCase):
    def test_unknown_reasons_are_exact_enum_values(self):
        cases = [
            ("@Mattilsynet/provetaking", (self.team,),
             Provenance("Mattilsynet", "fixture", Coverage.UNAUTHORIZED), "unauthorized"),
            ("@Mattilsynet/absent", (self.team,), self.source, "not_observed"),
            ("@Mattilsynet/provetaking", (self.team, TeamDto("provetaking", "Other")),
             self.source, "ambiguous_identity"),
            ("invalid", (self.team,), self.source, "invalid_owner"),
        ]
        cases = [(owner, teams, source, self.limits, expected)
                 for owner, teams, source, expected in cases]
        cases.extend([
            ("@Mattilsynet/provetaking", (self.team,), self.source, None, "invalid_limits"),
            ("@Mattilsynet/provetaking", (self.team,), None, self.limits, "invalid_provenance"),
            ("@Mattilsynet/provetaking", (self.team,), Provenance("Mattilsynet", "", Coverage.COMPLETE), self.limits, "invalid_provenance"),
            ("@Mattilsynet/provetaking", [], self.source, self.limits, "catalog_bound_or_shape"),
            ("@Mattilsynet/provetaking", (self.team,), self.source, Limits(0, 128), "catalog_bound_or_shape"),
            ("@Other/provetaking", (self.team,), self.source, self.limits, "different_organization"),
            ("@Mattilsynet/provetaking", (None,), self.source, self.limits, "invalid_catalog_entry"),
            ("@Mattilsynet/provetaking", (TeamDto("bad/slug", "name"),), self.source, self.limits, "invalid_catalog_entry"),
            ("@Mattilsynet/provetaking", (TeamDto("provetaking", ""),), self.source, self.limits, "invalid_catalog_entry"),
        ])
        self.assertEqual({UnknownReason(case[-1]) for case in cases}, set(UnknownReason))
        for owner, teams, source, limits, expected in cases:
            with self.subTest(expected=expected):
                result = match_owner(owner, teams, source, limits)
                self.assertIsInstance(result, Unknown)
                self.assertIs(result.reason, UnknownReason(expected))

    def test_unknown_constructor_requires_reason_enum(self):
        for reason in UnknownReason:
            self.assertIs(Unknown(reason).reason, reason)
        for invalid in ("unauthorized", None, 1, Coverage.UNAUTHORIZED):
            with self.subTest(invalid=invalid), self.assertRaises(TypeError):
                Unknown(invalid)
        with self.assertRaises(TypeError):
            replace(Unknown(UnknownReason.NOT_OBSERVED), reason="not_observed")
        with self.assertRaises(FrozenInstanceError):
            Unknown(UnknownReason.NOT_OBSERVED).reason = "not_observed"

    def test_catalog_validation_intentionally_stops_at_first_failure(self):
        conflict = TeamDto(self.team.slug, "Other")
        for teams, expected in [
            ((self.team, conflict, None), UnknownReason.AMBIGUOUS_IDENTITY),
            ((None, self.team, conflict), UnknownReason.INVALID_CATALOG_ENTRY),
            ((self.team, None, conflict), UnknownReason.INVALID_CATALOG_ENTRY),
        ]:
            with self.subTest(teams=teams):
                self.assertEqual(self.match("@Mattilsynet/provetaking", teams), Unknown(expected))

    def test_instant_constructor_and_replace_reject_negative_elapsed(self):
        self.assertEqual(Instant(timedelta()).elapsed, timedelta())
        self.assertEqual(Instant(timedelta(microseconds=1)).elapsed, timedelta(microseconds=1))
        with self.assertRaisesRegex(ValueError, "nonnegative"):
            Instant(timedelta(microseconds=-1))
        with self.assertRaisesRegex(ValueError, "nonnegative"):
            replace(Instant(timedelta()), elapsed=timedelta(seconds=-1))
        with self.assertRaises(FrozenInstanceError):
            Instant(timedelta()).elapsed = timedelta(seconds=-1)

    def test_incoherent_observer_clock_is_explicit_error(self):
        instant = Instant(timedelta(seconds=1))
        with self.assertRaisesRegex(ValueError, "before last progress"):
            observe(Running(instant), Instant(timedelta()))
        self.assertEqual(observe(Running(instant), instant), ProgressAge(timedelta()))
        self.assertEqual(observe(Running(instant), instant.after(timedelta(seconds=1))),
                         ProgressAge(timedelta(seconds=1)))

    def test_identical_entries_deduplicate_without_hiding_conflicts(self):
        duplicate = TeamDto(self.team.slug, self.team.display_name)
        for owner, kind in [("@Mattilsynet/provetaking", Exact),
                            ("@Mattilsynet/PROVETAKING", CaseDiscrepancy)]:
            with self.subTest(owner=owner):
                self.assertEqual(self.match(owner, (self.team, duplicate)),
                                 kind(owner, self.team, self.source))
        for conflict in [TeamDto("provetaking", "Other"),
                         TeamDto("Provetaking", self.team.display_name)]:
            for teams in [(self.team, duplicate, conflict), (conflict, duplicate, self.team)]:
                self.assertEqual(self.match("@Mattilsynet/provetaking", teams),
                                 Unknown(UnknownReason.AMBIGUOUS_IDENTITY))

    def setUp(self):
        self.team = TeamDto("provetaking", "Prøvetaking")
        self.source = Provenance("Mattilsynet", "fixture-observation", Coverage.PARTIAL_VISIBILITY)
        self.limits = Limits(37, 128)

    def match(self, owner, teams=None, source=None, limits=None):
        return match_owner(owner, (self.team,) if teams is None else teams,
                           source or self.source, limits or self.limits)

    def test_exact_observed_slug_is_positive_under_partial_visibility(self):
        self.assertEqual(self.match("@Mattilsynet/provetaking"),
                         Exact("@Mattilsynet/provetaking", self.team, self.source))

    def test_case_discrepancy_preserves_raw_and_observed_spelling(self):
        self.assertEqual(self.match("@mattilsynet/Provetaking"),
                         CaseDiscrepancy("@mattilsynet/Provetaking", self.team, self.source))

    def test_display_name_is_never_used_to_guess_slug(self):
        self.assertIsInstance(self.match("@Mattilsynet/Prøvetaking"), Unknown)

    def test_secret_team_omitted_from_37_visible_is_unknown(self):
        visible = tuple(TeamDto(f"visible-{n}", f"Visible {n}") for n in range(37))
        for coverage in Coverage:
            with self.subTest(coverage=coverage):
                source = Provenance("Mattilsynet", "fixture-only", coverage)
                self.assertIsInstance(self.match("@Mattilsynet/secret-team", visible, source), Unknown)

    def test_incomplete_positive_is_observed_not_completeness_claim(self):
        source = Provenance("Mattilsynet", "fixture-only", Coverage.INCOMPLETE)
        self.assertEqual(self.match("@Mattilsynet/provetaking", source=source),
                         Exact("@Mattilsynet/provetaking", self.team, source))

    def test_unauthorized_input_cannot_launder_a_positive(self):
        source = Provenance("Mattilsynet", "fixture-only", Coverage.UNAUTHORIZED)
        self.assertIsInstance(self.match("@Mattilsynet/provetaking", source=source), Unknown)

    def test_invalid_or_cross_org_tokens_are_unknown(self):
        for owner in (None, "", "@user", "@Other/provetaking", "@Mattilsynet/*",
                      "@Mattilsynet/a/b", "@Mattilsynet/", "@Mattilsynet/ provetaking"):
            with self.subTest(owner=owner):
                self.assertIsInstance(self.match(owner), Unknown)

    def test_ambiguous_observed_identities_do_not_select_first(self):
        teams = (self.team, TeamDto("Provetaking", "Other identity"))
        self.assertIsInstance(self.match("@Mattilsynet/PROVETAKING", teams), Unknown)

    def test_limits_at_boundary_and_over_limit(self):
        owner = "@Mattilsynet/provetaking"
        self.assertIsInstance(self.match(owner, limits=Limits(1, len(owner))), Exact)
        self.assertIsInstance(self.match(owner, limits=Limits(0, 128)), Unknown)
        self.assertIsInstance(self.match(owner, limits=Limits(1, len(owner) - 1)), Unknown)
        self.assertIsInstance(self.match(owner, (TeamDto("provetaking", "x" * 129),)), Unknown)

    def test_catalog_shape_and_metadata_are_not_silently_trusted(self):
        self.assertIsInstance(self.match("@Mattilsynet/provetaking", iter((self.team,))), Unknown)
        source = Provenance("Mattilsynet", "", Coverage.PARTIAL_VISIBILITY)
        self.assertIsInstance(self.match("@Mattilsynet/provetaking", source=source), Unknown)

    def test_malicious_display_name_remains_data_for_future_ui_contract(self):
        team = TeamDto("provetaking", '<img src=x onerror="alert(1)"> &')
        result = self.match("@Mattilsynet/provetaking", (team,))
        self.assertIsInstance(result, Exact)
        self.assertEqual(escape(result.team.display_name, quote=True),
                         '&lt;img src=x onerror=&quot;alert(1)&quot;&gt; &amp;')


if __name__ == "__main__":
    unittest.main()
