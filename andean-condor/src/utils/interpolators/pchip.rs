/// Maximum squared sum of normalized derivatives for PCHIP monotonicity
/// constraint. If alpha^2 + beta^2 > 9, the derivatives are scaled down to
/// preserve monotonicity.
const PCHIP_MAX_TAU_SQUARED: f64 = 9.0;

#[inline]
pub fn pchip(x: &[f64; 4], y: &[f64; 4], xi: f64) -> Option<f64> {
    // Check strictly increasing
    for i in 0..3 {
        if x[i + 1] <= x[i] {
            return None;
        }
    }

    // Find interval containing xi
    let mut k = 0;
    for i in 0..3 {
        if xi >= x[i] && xi <= x[i + 1] {
            k = i;
            break;
        }
    }

    // Calculate slopes
    let s0 = (y[1] - y[0]) / (x[1] - x[0]);
    let s1 = (y[2] - y[1]) / (x[2] - x[1]);
    let s2 = (y[3] - y[2]) / (x[3] - x[2]);

    // Calculate derivatives using PCHIP method
    let mut d = [0.0; 4];

    // Endpoint derivatives
    d[0] = s0;
    d[3] = s2;

    // Interior derivatives (weighted harmonic mean)
    #[expect(clippy::needless_range_loop)]
    for i in 1..=2 {
        let (s_prev, s_next, h_prev, h_next) = if i == 1 {
            (s0, s1, x[1] - x[0], x[2] - x[1])
        } else {
            (s1, s2, x[2] - x[1], x[3] - x[2])
        };

        if s_prev * s_next <= 0.0 {
            d[i] = 0.0;
        } else {
            let w1 = 2.0f64.mul_add(h_next, h_prev);
            let w2 = 2.0f64.mul_add(h_prev, h_next);
            d[i] = (w1 + w2) / (w1 / s_prev + w2 / s_next);
        }
    }

    // Monotonicity constraint
    let slopes = [s0, s1, s2];
    for i in 0..3 {
        if slopes[i] == 0.0 {
            d[i] = 0.0;
            d[i + 1] = 0.0;
        } else {
            let alpha = d[i] / slopes[i];
            let beta = d[i + 1] / slopes[i];
            let tau = alpha.mul_add(alpha, beta * beta);

            if tau > PCHIP_MAX_TAU_SQUARED {
                let scale = 3.0 / tau.sqrt();
                d[i] = scale * alpha * slopes[i];
                d[i + 1] = scale * beta * slopes[i];
            }
        }
    }

    // Hermite cubic evaluation
    let h = x[k + 1] - x[k];
    let t = (xi - x[k]) / h;
    let t2 = t * t;
    let t3 = t2 * t;

    // (2.0 * t3 - 3.0 * t2 + 1.0) * y[k]
    // + (t3 - 2.0 * t2 + t) * h * d[k]
    // + (-2.0 * t3 + 3.0 * t2) * y[k + 1]
    // + (t3 - t2) * h * d[k + 1],
    Some(
        (2.0f64.mul_add(t3, -(3.0 * t2)) + 1.0)
            .mul_add(y[k], (2.0f64.mul_add(-t2, t3) + t) * h * d[k])
            + (-2.0f64).mul_add(t3, 3.0 * t2).mul_add(y[k + 1], (t3 - t2) * h * d[k + 1]),
    )
}

#[cfg(test)]
mod tests {
    use super::pchip as interpolate_pchip;

    #[test]
    fn pchip() {
        // Test with monotonic data
        // CRF 5 (92.4354), CRF 15 (85.7452), CRF 25 (80.5088), CRF 35 (72.9709)
        let x = [72.9709, 80.5088, 85.7452, 92.4354]; // scores (ascending order)
        let y = [35.0, 25.0, 15.0, 5.0]; // CRFs

        // Test exact points
        assert!(
            (interpolate_pchip(&x, &y, 72.9709).expect("result should exist") - 35.0).abs() < 1e-10
        );
        assert!(
            (interpolate_pchip(&x, &y, 80.5088).expect("result should exist") - 25.0).abs() < 1e-10
        );
        assert!(
            (interpolate_pchip(&x, &y, 85.7452).expect("result should exist") - 15.0).abs() < 1e-10
        );
        assert!(
            (interpolate_pchip(&x, &y, 92.4354).expect("result should exist") - 5.0).abs() < 1e-10
        );

        // Test interpolation for score 89.0
        let result = interpolate_pchip(&x, &y, 89.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 5.0
                && result.expect("result should exist") < 15.0
        );

        // Test with data that has varying slopes
        // CRF 40 (66.699707), CRF 45 (57.916622), CRF 50 (50.740498), CRF 55
        // (37.303120)
        let x2 = [37.303120, 50.740498, 57.916622, 66.699707]; // scores (ascending order)
        let y2 = [55.0, 50.0, 45.0, 40.0]; // CRFs

        // Should handle the steep changes in score
        let result = interpolate_pchip(&x2, &y2, 54.0);
        assert!(result.is_some());
        assert!(
            result.expect("result should exist") > 45.0
                && result.expect("result should exist") < 50.0
        );

        // Test with non-increasing x values (should return None)
        let x_bad = [72.9709, 88.0, 85.7452, 92.4354]; // Not properly ordered
        let y_bad = [35.0, 12.0, 15.0, 5.0];
        assert_eq!(interpolate_pchip(&x_bad, &y_bad, 87.0), None);

        // Test edge case with nearly flat region
        // CRF 63-66 have very similar scores
        let x_flat = [4.944567, 5.270722, 5.345044, 5.575547]; // scores (ascending order)
        let y_flat = [65.0, 66.0, 64.0, 63.0]; // CRFs
        let result = interpolate_pchip(&x_flat, &y_flat, 5.1);
        assert!(result.is_some());
        // Should handle the nearly flat region gracefully
    }
}
