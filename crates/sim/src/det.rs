//! Deterministic float helpers. The ONLY place the sim gets maths beyond
//! `+ - * /`, `sqrt`, `abs`, `floor`-via-libm and comparisons.
//!
//! Rust 1.95's own docs (`std/primitive.f32.html`) mark 36 `f32` methods as
//! non-deterministic. 33 are maths functions whose precision "varies by
//! platform, Rust version, and can even differ within the same execution"
//! (sin, cos, tanh, exp, atan2, powi, to_radians, ...). The other three are
//! `max`, `min` and `clamp`: given +0.0 and -0.0 they may return either one,
//! and the two zeros hash differently. `tests/float_hygiene.rs` bans all 36
//! names from the sim source, so everything goes through here.

pub const PI: f32 = core::f32::consts::PI;
pub const TAU: f32 = 2.0 * PI;
/// Degrees to radians by plain multiply (`to_radians` is on the banned list).
pub const DEG: f32 = PI / 180.0;

#[inline]
pub fn sin(x: f32) -> f32 {
    libm::sinf(x)
}
#[inline]
pub fn cos(x: f32) -> f32 {
    libm::cosf(x)
}
#[inline]
pub fn tanh(x: f32) -> f32 {
    libm::tanhf(x)
}
#[inline]
pub fn floor(x: f32) -> f32 {
    libm::floorf(x)
}

/// `a` if `a > b`, else `b`. Ties return `b`, always.
#[inline]
pub fn max(a: f32, b: f32) -> f32 {
    if a > b {
        a
    } else {
        b
    }
}
/// `a` if `a < b`, else `b`. Ties return `b`, always.
#[inline]
pub fn min(a: f32, b: f32) -> f32 {
    if a < b {
        a
    } else {
        b
    }
}
#[inline]
pub fn clamp(x: f32, lo: f32, hi: f32) -> f32 {
    min(max(x, lo), hi)
}

/// Wrap an angle into [-PI, PI). The input is at most a few radians out.
#[inline]
pub fn wrap_angle(mut a: f32) -> f32 {
    while a >= PI {
        a -= TAU;
    }
    while a < -PI {
        a += TAU;
    }
    a
}

/// Wrap a coordinate onto a torus of size `size`. Moves per tick are far
/// smaller than the world, so one correction suffices, but loop to be safe.
#[inline]
pub fn wrap(mut v: f32, size: f32) -> f32 {
    while v >= size {
        v -= size;
    }
    while v < 0.0 {
        v += size;
    }
    v
}

/// Shortest signed difference `b - a` on a torus of size `size`.
#[inline]
pub fn torus_delta(a: f32, b: f32, size: f32) -> f32 {
    let mut d = b - a;
    let half = size * 0.5;
    if d > half {
        d -= size;
    } else if d < -half {
        d += size;
    }
    d
}
