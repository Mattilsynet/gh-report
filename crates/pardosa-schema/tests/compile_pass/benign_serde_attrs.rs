use pardosa_schema::{EventString, GenomeSafe, OrderedF64};
use serde::Serialize;
#[derive(GenomeSafe, Serialize)]
#[serde(rename = "MyPoint")]
struct Point {
    #[serde(rename = "x_coord")]
    x: OrderedF64,
    y: OrderedF64,
}
#[derive(GenomeSafe)]
struct Container {
    item_alpha: u32,
    item_beta: u32,
    item_gamma: u32,
    first_value: Option<EventString<256>>,
    second_value: Option<EventString<256>>,
}
fn main() {
    let _ = Point::SCHEMA_HASH;
    let _ = Container::SCHEMA_HASH;
}
