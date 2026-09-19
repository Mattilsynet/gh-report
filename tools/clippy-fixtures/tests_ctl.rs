#![crate_type = "lib"]
//! Controls for clippy.toml test-context configuration.
//!
//! Every `helper_*` item is an ordinary function and must always fire its
//! lint. Every `recognized_*` item is a real `#[test]` and is silent only
//! while the matching `allow-*-in-tests` setting is true.

pub fn helper_unwrap(value: Result<u32, ()>) -> u32 { value.unwrap() }

pub fn helper_expect(value: Result<u32, ()>) -> u32 { value.expect("helper") }

pub fn helper_panic() -> u32 { panic!("helper") }

pub fn helper_index(values: &[u32]) -> u32 { values[0] }

#[test]
fn recognized_unwrap() {
    let value: Result<u32, ()> = Ok(1);
    let taken = value.unwrap();
    assert_eq!(taken, 1);
}

#[test]
fn recognized_expect() {
    let value: Result<u32, ()> = Ok(2);
    let taken = value.expect("test");
    assert_eq!(taken, 2);
}

#[test]
fn recognized_panic() {
    let ok = true;
    if !ok {
        panic!("test");
    }
}

#[test]
fn recognized_index() {
    let numbers: &[u32] = &[3, 4];
    let first = numbers[0];
    assert_eq!(first, 3);
}
