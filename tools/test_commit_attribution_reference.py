import itertools
import unittest

from commit_attribution_reference import LinkedUser, Selected, System, Unknown, classify, select


def user(login):
    return {"login": login, "type": "User"}


class AttributionReferenceTests(unittest.TestCase):
    def test_web_flow_falls_back_to_linked_author(self):
        self.assertEqual(
            select({"committer": user("WEB-FLOW"), "author": user("alice")}),
            Selected("author", LinkedUser("alice")),
        )

    def test_committer_precedes_author(self):
        self.assertEqual(
            select({"committer": user("bob"), "author": user("alice")}),
            Selected("committer", LinkedUser("bob")),
        )

    def test_display_name_and_privacy_email_are_not_identity(self):
        metadata = {"name": "GitHub", "email": "alice@users.noreply.github.com"}
        self.assertEqual(classify(user("alice") | metadata), LinkedUser("alice"))
        self.assertIsInstance(classify(metadata), Unknown)
        self.assertIsInstance(select({"author": metadata}), Unknown)

    def test_bot_signals_and_missing_type(self):
        for raw in (user("ci[bot]"), {"type": "Bot", "login": "ci"}):
            with self.subTest(raw=raw):
                self.assertIsInstance(classify(raw), System)
        for raw in (None, {}, [], "alice", {"login": "alice"},
                    {"login": "alice", "type": "FutureActor"}):
            with self.subTest(raw=raw):
                self.assertIsInstance(classify(raw), Unknown)

    def test_unusable_login_never_becomes_link(self):
        for login in (None, "", "  ", "alice/bob", "../alice", "a?b", 7, "ålice"):
            with self.subTest(login=login):
                self.assertIsInstance(classify(user(login)), Unknown)

    def test_no_false_attribution_for_all_unlinked_pairs(self):
        candidates = (None, {}, user("web-flow"), user("ci[bot]"),
                      {"type": "Bot"}, {"name": "alice"},
                      {"email": "123+alice@users.noreply.github.com"})
        for committer, author in itertools.product(candidates, repeat=2):
            with self.subTest(committer=committer, author=author):
                self.assertIsInstance(select({"committer": committer, "author": author}), Unknown)

    def test_unknown_committer_does_not_hide_linked_author(self):
        self.assertEqual(select({"author": user("alice")}), Selected("author", LinkedUser("alice")))

    def test_missing_commit_is_not_negative_identity_evidence(self):
        for raw in (None, [], "", 404):
            with self.subTest(raw=raw):
                self.assertIsInstance(select(raw), Unknown)


if __name__ == "__main__":
    unittest.main()
