# SBOM merge fixtures

Hand-shaped inputs for `tools/test_sbom_merge.py`. Small and deliberate, not
captured from the real workspace: the checked-in shape mirrors what
`cargo metadata --filter-platform x86_64-unknown-linux-gnu` and
`cargo-cyclonedx 0.5.7` emit (Cargo PackageId `bom-ref`s, `pkg:cargo` purls,
workspace members carrying `download_url=file://...` qualifiers).

`image-bom.json` is a **documented shape fixture, not a claimed scan**. The
release base image is distroless `gcr.io/distroless/cc-debian12:nonroot`,
which ships no OS package database, so a real scan of the pushed digest yields
approximately zero OS components. `syft` is not installed on the development
host (only `podman` is), so no local scan was produced. The fixture therefore
carries two synthetic `pkg:deb` components and one purl-less `file` component
to exercise retention, counting, and the purl-less drop rule. The real release
workflow replaces it with an actual `syft` scan of the pushed digest.
