//! Host-pure sparkline point math: maps a bounded, ordered sequence of
//! `f64` samples onto SVG `viewBox` coordinates. App-agnostic (C6) —
//! it knows nothing of [`crate::sd::LevelHistory`], [`crate::sim`], or
//! any queue vocabulary, only "an ordered slice of samples, oldest to
//! newest". Kept out of `sd.rs` so the generic SD core stays free of
//! rendering-domain concerns; kept out of the `wasm32`-gated component
//! module (C1/C7) so this math is host-testable without a `wasm32`
//! target.

/// Maps `samples` (oldest to newest) onto an SVG `viewBox` of
/// `width` x `height`, returning `"x,y x,y ..."` ready for a
/// `<polyline points=...>` attribute.
///
/// - Empty `samples` yields an empty string (nothing to draw).
/// - A single sample lands at `x = width / 2.0` (no span to
///   distribute across two or more points).
/// - Otherwise the oldest sample lands at `x = 0.0`, the newest at
///   `x = width`, evenly spaced between.
/// - `y` autoscales against the largest sample in `samples`: that
///   sample lands at `y = 0.0` (top of the viewBox); a sample of
///   `0.0` lands at `y = height` (bottom). When every sample is
///   `<= 0.0` (no positive maximum to scale against), every point
///   lands at `y = height`.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "sample counts and indices are bounded well under 2^52 for any realistic history capacity"
)]
pub fn polyline_points(samples: &[f64], width: f64, height: f64) -> String {
    let Some(&first) = samples.first() else {
        return String::new();
    };
    let max = max_sample(samples);
    if samples.len() == 1 {
        return format!("{},{}", width / 2.0, scaled_y(first, max, height));
    }
    let last_index = (samples.len() - 1) as f64;
    samples
        .iter()
        .enumerate()
        .map(|(index, &level)| {
            let x = (index as f64 / last_index) * width;
            format!("{x},{}", scaled_y(level, max, height))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn max_sample(samples: &[f64]) -> f64 {
    samples.iter().copied().fold(f64::MIN, f64::max)
}

fn scaled_y(level: f64, max: f64, height: f64) -> f64 {
    if max <= 0.0 {
        return height;
    }
    height - (level / max) * height
}

#[cfg(test)]
mod tests {
    use super::polyline_points;

    #[test]
    fn empty_history_yields_empty_string() {
        assert_eq!(polyline_points(&[], 200.0, 50.0), "");
    }

    #[test]
    fn single_sample_lands_at_half_width() {
        let points = polyline_points(&[10.0], 200.0, 50.0);
        assert_eq!(points, "100,0");
    }

    #[test]
    fn full_window_spans_x_from_zero_to_width() {
        let samples = [1.0, 2.0, 3.0, 4.0, 5.0];
        let points = polyline_points(&samples, 200.0, 50.0);
        let coords: Vec<&str> = points.split(' ').collect();
        assert_eq!(coords.len(), 5);
        assert!(coords[0].starts_with("0,"));
        assert!(coords[4].starts_with("200,"));
    }

    #[test]
    fn scales_to_max_level() {
        let samples = [0.0, 5.0, 10.0];
        let points = polyline_points(&samples, 200.0, 50.0);
        let coords: Vec<&str> = points.split(' ').collect();
        assert_eq!(coords[0], "0,50");
        assert_eq!(coords[2], "200,0");
    }

    #[test]
    fn all_non_positive_samples_land_at_bottom() {
        let samples = [-3.0, -1.0, 0.0];
        let points = polyline_points(&samples, 200.0, 50.0);
        for coord in points.split(' ') {
            assert!(
                coord.ends_with(",50"),
                "expected baseline y=50, got {coord}"
            );
        }
    }
}
