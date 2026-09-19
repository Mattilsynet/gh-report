#![crate_type = "lib"]

#[derive(Debug)]
pub enum LoadError {
    Missing,
    Corrupt,
    Denied,
}

#[must_use]
pub fn checked_handle() -> u32 {
    7
}

pub fn parse(raw: &str) -> Result<u32, LoadError> {
    if raw.is_empty() {
        panic!("empty input");
    }
    Ok(raw.len() as u32)
}

pub fn unwrapping(raw: Result<u32, LoadError>) -> Result<u32, LoadError> {
    let value = raw.unwrap();
    Ok(value)
}

pub fn expecting(raw: Option<u32>) -> Result<u32, LoadError> {
    Ok(raw.expect("present"))
}

pub fn discard_must_use() {
    let _ = checked_handle();
}

pub fn drop_result(value: Result<u32, LoadError>) {
    value.ok();
}

pub fn lossy_map(value: Result<u32, LoadError>) -> Result<u32, ()> {
    value.map_err(|_| ())
}

pub fn wildcard(err: &LoadError) -> &'static str {
    match err {
        LoadError::Missing => "missing",
        _ => "other",
    }
}

pub fn sliced(values: &[u32]) -> u32 {
    values[0]
}
