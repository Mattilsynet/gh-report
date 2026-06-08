use crate::error::DomainError;
#[cfg(test)]
use crate::genome_safe::GenomeSafe;
#[cfg(test)]
use pardosa_wire::Validate;
#[inline]
pub(super) fn reject_non_real_f32(v: f32) -> Result<f32, DomainError> {
    if v.is_nan() || v.is_infinite() || v.is_subnormal() {
        return Err(DomainError::NotReal);
    }
    if v == 0.0_f32 && v.is_sign_negative() {
        return Ok(0.0_f32);
    }
    Ok(v)
}
#[inline]
pub(super) fn reject_non_real_f64(v: f64) -> Result<f64, DomainError> {
    if v.is_nan() || v.is_infinite() || v.is_subnormal() {
        return Err(DomainError::NotReal);
    }
    if v == 0.0_f64 && v.is_sign_negative() {
        return Ok(0.0_f64);
    }
    Ok(v)
}
/// Stable u8 discriminant for [`EventF32`] / [`EventF64`] wire tags.
///
/// Pinned by `event_f32_layout_tags` / `event_f64_layout_tags`. Adding a
/// new variant is ADR-0009 breaking (`SCHEMA_HASH` changes); reusing an
/// existing discriminant is forbidden.
pub(super) mod event_float_tag {
    pub(super) const NAN: u8 = 0;
    pub(super) const NEG_INF: u8 = 1;
    pub(super) const FINITE: u8 = 2;
    pub(super) const POS_INF: u8 = 3;
}
pub(crate) mod event;
pub(crate) mod ordered;
pub(crate) mod real;
pub use event::{EventF32, EventF64};
pub use ordered::{OrderedF32, OrderedF64};
pub use real::{RealF32, RealF64};
#[cfg(test)]
mod tests;
