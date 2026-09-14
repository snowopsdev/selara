//! Pure geometry for Selara's native working indicator.
//!
//! This is a bounded Rust port of the `orbits` mode from thinking-orbs
//! v0.3.1 (revision `bd204b73c9b6660fad7210b1ad48d9dc2adbb89d`). The source
//! is MIT licensed; see `vendor/thinking-orbs/LICENSE`.

use std::cmp::Ordering;

const SIZE: f64 = 64.0;
const SPEED: f64 = 1.885;
const ORBIT_COUNT: usize = 12;
const GHOST_COUNT: usize = 40;
const PARTICLE_COUNT: usize = 3;
const GHOST_RADIUS: f64 = 0.9;
const GHOST_ALPHA: f64 = 0.5;
const PARTICLE_RADIUS: f64 = 1.2;
const PARTICLE_RADIUS_DEPTH: f64 = 1.6;
const RADIUS_POWER: f64 = 0.6;
const MIN_RADIUS: f64 = 0.3;

/// A single finished draw instruction for the working orb.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Dot {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) z: f64,
    pub(crate) radius: f64,
    pub(crate) white: f64,
    pub(crate) alpha: f64,
}

/// Build one 64px working frame for elapsed time in seconds.
///
/// The returned dots are in painter order: farthest first (`z` ascending).
/// The point count is fixed at 516, so this function remains bounded for all
/// finite elapsed times supplied by the progress panel.
pub(crate) fn working_frame(seconds: f64) -> Vec<Dot> {
    let center = SIZE / 2.0;
    let orbit_radius = center * 0.82;
    let time = seconds * SPEED;
    let radius_scale = (SIZE / 300.0).powf(RADIUS_POWER);
    let tilt: f64 = 0.3;
    let sin_tilt = tilt.sin();
    let cos_tilt = tilt.cos();
    let yaw = time * 0.12;
    let sin_yaw = yaw.sin();
    let cos_yaw = yaw.cos();

    let mut dots = Vec::with_capacity(ORBIT_COUNT * (GHOST_COUNT + PARTICLE_COUNT));

    for orbit in 0..ORBIT_COUNT {
        let orbit_f = orbit as f64;
        let h1 = hash_d(orbit_f, 1.7);
        let h2 = hash_d(orbit_f, 5.2);
        let h3 = hash_d(orbit_f, 8.9);
        let radius = orbit_radius * (0.45 + 0.52 * h1);
        let theta = h1 * 2.0 * std::f64::consts::PI;
        let phi = (2.0 * h2 - 1.0).acos();

        // Build an orthonormal basis for the orbit plane. This follows the
        // upstream cross-product order, which also fixes each orbit's phase.
        let nx = phi.sin() * theta.cos();
        let ny = phi.cos();
        let nz = phi.sin() * theta.sin();
        let mut ux = -ny;
        let mut uy = nx;
        let uz = 0.0;
        let basis_length = (ux * ux + uy * uy).sqrt().max(1e-6);
        ux /= basis_length;
        uy /= basis_length;
        let vx = -nz * uy;
        let vy = nz * ux;
        let vz = nx * uy - ny * ux;
        let particle_speed = (0.25 + 0.55 * h3) * if h3 > 0.5 { 1.0 } else { -1.0 };

        for point in 0..GHOST_COUNT {
            let angle = point as f64 / GHOST_COUNT as f64 * 2.0 * std::f64::consts::PI;
            let (x, y, z) = orbit_point(
                ux, uy, uz, vx, vy, vz, angle, radius, sin_yaw, cos_yaw, sin_tilt, cos_tilt, center,
            );
            let depth = (z / radius + 1.0) / 2.0;
            dots.push(Dot {
                x,
                y,
                z,
                radius: (GHOST_RADIUS * radius_scale).max(MIN_RADIUS),
                white: 0.72,
                alpha: GHOST_ALPHA * (0.4 + 0.6 * depth),
            });
        }

        for particle in 0..PARTICLE_COUNT {
            let angle = time * particle_speed
                + particle as f64 / PARTICLE_COUNT as f64 * 2.0 * std::f64::consts::PI
                + h2 * 6.0;
            let (x, y, z) = orbit_point(
                ux, uy, uz, vx, vy, vz, angle, radius, sin_yaw, cos_yaw, sin_tilt, cos_tilt, center,
            );
            let depth = (z / radius + 1.0) / 2.0;
            dots.push(Dot {
                x,
                y,
                z,
                radius: ((PARTICLE_RADIUS + PARTICLE_RADIUS_DEPTH * depth) * radius_scale)
                    .max(MIN_RADIUS),
                white: 0.3 - 0.22 * depth,
                alpha: 1.0,
            });
        }
    }

    // `finalizeFrame` in the upstream engine uses a stable ascending z sort.
    // All values here are finite for normal elapsed times; retain equal-z
    // insertion order if a platform's math library produces a tie.
    dots.sort_by(|a, b| a.z.partial_cmp(&b.z).unwrap_or(Ordering::Equal));
    dots
}

#[allow(clippy::too_many_arguments)]
fn orbit_point(
    ux: f64,
    uy: f64,
    uz: f64,
    vx: f64,
    vy: f64,
    vz: f64,
    angle: f64,
    radius: f64,
    sin_yaw: f64,
    cos_yaw: f64,
    sin_tilt: f64,
    cos_tilt: f64,
    center: f64,
) -> (f64, f64, f64) {
    let (sin_angle, cos_angle) = angle.sin_cos();
    let x = (ux * cos_angle + vx * sin_angle) * radius;
    let y = (uy * cos_angle + vy * sin_angle) * radius;
    let z = (uz * cos_angle + vz * sin_angle) * radius;

    // Shared spin + tilt + orthographic projection from thinking-orbs/core.ts.
    let x1 = x * cos_yaw + z * sin_yaw;
    let z1 = -x * sin_yaw + z * cos_yaw;
    let y1 = y * cos_tilt - z1 * sin_tilt;
    let z2 = y * sin_tilt + z1 * cos_tilt;
    (center + x1, center - y1, z2)
}

/// Deterministic hash in [0, 1), matching JavaScript's `hashD`.
fn hash_d(a: f64, b: f64) -> f64 {
    let h = (a * 12.9898 + b * 78.233).sin() * 43_758.545_3;
    h - h.floor()
}

#[cfg(test)]
mod tests {
    use super::{working_frame, Dot};

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-11, "{actual} != {expected}");
    }

    fn assert_dot(actual: Dot, expected: [f64; 6]) {
        assert_close(actual.x, expected[0]);
        assert_close(actual.y, expected[1]);
        assert_close(actual.z, expected[2]);
        assert_close(actual.radius, expected[3]);
        assert_close(actual.white, expected[4]);
        assert_close(actual.alpha, expected[5]);
    }

    #[test]
    fn matches_upstream_golden_samples_at_representative_times() {
        let frame = working_frame(0.6);
        assert_eq!(frame.len(), 516);
        assert_dot(
            frame[0],
            [
                30.85074971902107,
                30.691517973153513,
                -24.109923868135848,
                0.35618673453649413,
                0.72,
                0.20038979159114176,
            ],
        );
        assert_dot(
            frame[11],
            [
                32.54824772734648,
                43.05608812199291,
                -21.92097341042544,
                0.508906418494157,
                0.2881905818429647,
                1.0,
            ],
        );
        assert_dot(
            frame[249],
            [
                25.967298195033628,
                10.274430608010768,
                -0.52241000699489,
                0.784192429387529,
                0.192547929984002,
                1.0,
            ],
        );
        assert_dot(
            frame[515],
            [
                33.14925028097893,
                33.30848202684649,
                24.10992386813585,
                0.35618673453649413,
                0.72,
                0.4996102084088583,
            ],
        );

        let frame = working_frame(1.7);
        assert_eq!(frame.len(), 516);
        assert_dot(
            frame[1],
            [
                30.58939199395569,
                32.17651539010626,
                -24.130900474714085,
                0.4754636452459916,
                0.2998096085729914,
                1.0,
            ],
        );
        assert_dot(
            frame[274],
            [
                24.08677412883976,
                46.77302833569307,
                1.115720964323365,
                0.812557777139638,
                0.182692953804467,
                1.0,
            ],
        );
        assert_dot(
            frame[515],
            [
                31.555783460568627,
                31.42658607208282,
                24.161854202100955,
                0.35618673453649413,
                0.72,
                0.49993245364404665,
            ],
        );
    }

    #[test]
    fn frame_is_finite_bounded_and_far_to_near() {
        let frame = working_frame(3.3);
        assert_eq!(frame.len(), 12 * (40 + 3));
        for dot in &frame {
            assert!(dot.x.is_finite() && dot.y.is_finite() && dot.z.is_finite());
            assert!(dot.radius.is_finite() && dot.radius >= 0.3);
            assert!((0.0..=1.0).contains(&dot.white));
            assert!((0.0..=1.0).contains(&dot.alpha));
            assert!((-1.0..=65.0).contains(&dot.x));
            assert!((-1.0..=65.0).contains(&dot.y));
            assert!((-27.0..=27.0).contains(&dot.z));
        }
        for pair in frame.windows(2) {
            assert!(pair[0].z <= pair[1].z);
        }
    }
}
