import copy
from pathlib import Path
import unittest

from workflow_subset import load, read_workflow


ROOT = Path(__file__).resolve().parents[1]
SHA = "${{ github.sha }}"
TAG = "${{ github.ref_name }}"
REVISION = "org.opencontainers.image.revision"


def assert_source_identity(test, workflow):
    steps = workflow["jobs"]["build"]["steps"]
    checkout = next(step for step in steps if step.get("uses", "").startswith("actions/checkout@"))
    build = next(step for step in steps if step.get("id") == "build-push")["with"]
    test.assertEqual(checkout.get("with", {}).get("ref"), SHA)
    test.assertEqual(build.get("labels", "").splitlines(), [f"{REVISION}={SHA}"])
    test.assertEqual(build["context"], ".")
    test.assertEqual(build["file"], "crates/gh-report/Dockerfile")


class ImageProvenance(unittest.TestCase):
    def setUp(self):
        self.workflow = read_workflow(ROOT / ".github/workflows/build.yml")

    def test_source_mutations_fail_then_original_passes(self):
        source = (ROOT / ".github/workflows/build.yml").read_text()
        for old, new in (("ref: " + SHA, "ref: " + TAG),
                         (REVISION + "=" + SHA, REVISION + "=" + TAG)):
            changed = source.replace(old, new, 1)
            self.assertNotEqual(changed, source)
            with self.assertRaises(AssertionError):
                assert_source_identity(self, load(changed))
            assert_source_identity(self, load(source))

    def test_source_identity_is_immutable_event_sha(self):
        assert_source_identity(self, self.workflow)

    def test_display_version_remains_separate(self):
        steps = self.workflow["jobs"]["build"]["steps"]
        build = next(step for step in steps if step.get("id") == "build-push")["with"]
        self.assertEqual(build["build-args"].splitlines(), [f"APP_VERSION={TAG}"])
        self.assertEqual(build["tags"].splitlines(), ["${{ env.IMAGE }}:latest", f"${{{{ env.IMAGE }}}}:{TAG}"])
        dockerfile = (ROOT / build["file"]).read_text()
        self.assertIn("\nARG APP_VERSION\n", dockerfile)
        self.assertIn('APP_VERSION="$APP_VERSION" cargo build --release --locked -p gh-report', dockerfile)
        self.assertNotIn(REVISION, dockerfile)

    def test_mutable_source_identity_is_rejected(self):
        for source in (TAG, "${{ github.ref }}", "latest", "v1.2.3", ""):
            for field in ("checkout", "label"):
                with self.subTest(source=source, field=field):
                    changed = copy.deepcopy(self.workflow)
                    steps = changed["jobs"]["build"]["steps"]
                    checkout = next(s for s in steps if s.get("uses", "").startswith("actions/checkout@"))
                    build = next(s for s in steps if s.get("id") == "build-push")["with"]
                    checkout["with"] = {"ref": SHA}
                    build["labels"] = f"{REVISION}={SHA}"
                    if field == "checkout":
                        checkout["with"]["ref"] = source
                    else:
                        build["labels"] = f"{REVISION}={source}"
                    with self.assertRaises(AssertionError):
                        assert_source_identity(self, changed)


if __name__ == "__main__":
    read_workflow(ROOT / ".github/workflows/build.yml")
    unittest.main()
