// SPDX-License-Identifier: GPL-3.0-or-later
//! Distance attenuation and stereo panning, ported from SVC's
//! `PositionalAudioUtils` (2.6.x).

use crate::state::Listener;

/// SVC's `getDistanceVolume`: a linear falloff to zero at `max_distance`.
pub fn distance_gain(distance: f64, max_distance: f32) -> f32 {
    let max = max_distance.max(0.001) as f64;
    let d = distance.min(max);
    ((1.0 - d / max) as f32).clamp(0.0, 1.0)
}

/// Constant-power left/right gains for a source at `source`.
pub fn gains(listener: Listener, source: [f64; 3], _max_distance: f32) -> (f32, f32) {
    let dx = source[0] - listener.position[0];
    let dy = source[1] - listener.position[1];
    let dz = source[2] - listener.position[2];

    let len = (dx * dx + dy * dy + dz * dz).sqrt();
    if len <= f64::EPSILON {
        return (1.0, 1.0);
    }
    let dir = [dx / len, dy / len, dz / len];

    // Java: Utils.angle(new Vec2(dir.x, dir.z), new Vec2(-1, 0)), i.e.
    // atan2(dot, cross) with cross = x1*y2 - y1*x2.
    let diff_angle = (-dir[0]).atan2(dir[2]).to_degrees() as f32;
    let angle = normalize_angle(diff_angle - listener.yaw);

    let vertical = ((listener.position[1] - source[1]).abs() / 32.0).min(1.0) as f32;
    let rot = angle / 180.0;
    let mut perc = if rot < -0.5 {
        -(0.5 + (rot + 0.5))
    } else if rot > 0.5 {
        0.5 - (rot - 0.5)
    } else {
        rot
    };
    perc *= 1.0 - vertical;

    let min_volume = 0.3f32;
    let mut left = if perc < 0.0 {
        (perc * 1.4).abs() + min_volume
    } else {
        min_volume
    };
    let mut right = if perc >= 0.0 {
        perc * 1.4 + min_volume
    } else {
        min_volume
    };
    let fill = 1.0 - left.max(right);
    left += fill;
    right += fill;

    let power = (left * left + right * right).sqrt();
    if power <= f32::EPSILON {
        return (1.0, 1.0);
    }

    // Never exceed unity: the distance gain is applied separately.
    let (left, right) = (left / power, right / power);
    let peak = left.max(right);
    if peak > 1.0 {
        (left / peak, right / peak)
    } else {
        (left, right)
    }
}

fn normalize_angle(angle: f32) -> f32 {
    let mut angle = angle % 360.0;
    if angle <= -180.0 {
        angle += 360.0;
    } else if angle > 180.0 {
        angle -= 360.0;
    }
    angle
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTENER: Listener = Listener {
        position: [0.0, 64.0, 0.0],
        yaw: 0.0,
    };

    #[test]
    fn a_source_at_the_listener_is_centred() {
        assert_eq!(gains(LISTENER, [0.0, 64.0, 0.0], 48.0), (1.0, 1.0));
    }

    #[test]
    fn a_source_in_front_is_centred() {
        let (left, right) = gains(LISTENER, [0.0, 64.0, 5.0], 48.0);
        assert!((left - right).abs() < 0.02, "{left} {right}");
        // Constant-power centre: 1/sqrt(2) per channel.
        assert!((left - 0.707).abs() < 0.02, "{left} {right}");
    }

    #[test]
    fn a_source_to_the_east_is_on_the_left_when_facing_south() {
        let (left, right) = gains(LISTENER, [5.0, 64.0, 0.0], 48.0);
        assert!(left > right + 0.2, "left {left}, right {right}");
    }

    #[test]
    fn a_source_above_is_less_panned() {
        let low = gains(LISTENER, [5.0, 64.0, 0.0], 48.0);
        let high = gains(LISTENER, [5.0, 96.0, 0.0], 48.0);
        assert!((high.0 - high.1).abs() < (low.0 - low.1).abs());
    }

    #[test]
    fn distance_gain_falls_linearly_to_zero() {
        assert_eq!(distance_gain(0.0, 48.0), 1.0);
        assert!((distance_gain(24.0, 48.0) - 0.5).abs() < 1e-6);
        assert_eq!(distance_gain(100.0, 48.0), 0.0);
    }

    #[test]
    fn yaw_turns_the_stereo_image() {
        // Facing east (yaw -90): a source east is now in front.
        let listener = Listener {
            yaw: -90.0,
            ..LISTENER
        };
        let (left, right) = gains(listener, [5.0, 64.0, 0.0], 48.0);
        assert!((left - right).abs() < 0.02, "{left} {right}");
    }

    #[test]
    fn the_result_is_finite_and_balanced() {
        for yaw in [-180.0, -90.0, 0.0, 45.0, 179.0] {
            for x in [-8.0, 0.0, 8.0] {
                let listener = Listener { yaw, ..LISTENER };
                let (l, r) = gains(listener, [x, 70.0, 3.0], 48.0);
                assert!(l.is_finite() && r.is_finite());
                assert!((l * l + r * r - 1.0).abs() < 1e-4, "{l} {r}");
            }
        }
    }
}
