#[test]
fn trybuild_compile_pass() {
    let t = trybuild::TestCases::new();
    t.pass("tests/trybuild/compile_pass/*.rs");
}
