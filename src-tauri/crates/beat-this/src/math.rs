//! Matrix products, and the exponential and error function the network
//! needs millions of times a second.

/// `c = a · bᵀ`, where `a` is m×k, `b` is n×k and `c` is m×n, row-major.
/// A linear layer over m tokens is this with `b` its weights.
pub fn mul_abt(m: usize, k: usize, n: usize, a: &[f32], b: &[f32], c: &mut [f32]) {
    assert!(a.len() >= m * k && b.len() >= n * k && c.len() >= m * n);
    if m == 0 || n == 0 || k == 0 {
        return;
    }
    // SAFETY: the three buffers hold the sizes the strides walk, checked above.
    unsafe { gemm(m, k, n, a.as_ptr(), b.as_ptr(), true, c.as_mut_ptr()) }
}

/// `c = a · b`, where `a` is m×k, `b` is k×n and `c` is m×n, row-major.
pub fn mul_ab(m: usize, k: usize, n: usize, a: &[f32], b: &[f32], c: &mut [f32]) {
    assert!(a.len() >= m * k && b.len() >= k * n && c.len() >= m * n);
    if m == 0 || n == 0 || k == 0 {
        return;
    }
    // SAFETY: as above.
    unsafe { gemm(m, k, n, a.as_ptr(), b.as_ptr(), false, c.as_mut_ptr()) }
}

#[cfg(all(target_os = "macos", not(feature = "portable")))]
unsafe fn gemm(m: usize, k: usize, n: usize, a: *const f32, b: *const f32, b_transposed: bool, c: *mut f32) {
    #[link(name = "Accelerate", kind = "framework")]
    extern "C" {
        fn cblas_sgemm(
            order: i32,
            trans_a: i32,
            trans_b: i32,
            m: i32,
            n: i32,
            k: i32,
            alpha: f32,
            a: *const f32,
            lda: i32,
            b: *const f32,
            ldb: i32,
            beta: f32,
            c: *mut f32,
            ldc: i32,
        );
    }
    const ROW_MAJOR: i32 = 101;
    const NO_TRANSPOSE: i32 = 111;
    const TRANSPOSE: i32 = 112;
    let (trans_b, ldb) = if b_transposed { (TRANSPOSE, k) } else { (NO_TRANSPOSE, n) };
    cblas_sgemm(ROW_MAJOR, NO_TRANSPOSE, trans_b, m as i32, n as i32, k as i32, 1.0, a, k as i32, b, ldb as i32, 0.0, c, n as i32);
}

#[cfg(not(all(target_os = "macos", not(feature = "portable"))))]
unsafe fn gemm(m: usize, k: usize, n: usize, a: *const f32, b: *const f32, b_transposed: bool, c: *mut f32) {
    // `b` transposed is the same memory walked with its strides swapped.
    let (rsb, csb) = if b_transposed { (1, k as isize) } else { (n as isize, 1) };
    matrixmultiply::sgemm(m, k, n, 1.0, a, k as isize, 1, b, rsb, csb, 0.0, c, n as isize, 1);
}

/// `e^x` to about seven digits, written so a loop of it vectorises: range
/// reduction to a power of two and a polynomial (Cephes' `expf`).
#[inline(always)]
#[allow(clippy::manual_clamp)] // `clamp` keeps a NaN, which the cast below must not see
pub fn exp(x: f32) -> f32 {
    // `max` then `min` also turns a NaN into a number, which the cast needs.
    let x = x.max(-87.0).min(88.0);
    let scaled = x * std::f32::consts::LOG2_E;
    let rounded = scaled + if scaled < 0.0 { -0.5 } else { 0.5 };
    // SAFETY: `x` is within ±88, so `rounded` is within ±128.
    let n = unsafe { rounded.to_int_unchecked::<i32>() };
    let whole = n as f32;
    let r = x - whole * 0.693_359_4 + whole * 2.121_944_4e-4;
    let mut p = 1.987_569_1e-4_f32;
    p = p * r + 1.398_199_9e-3;
    p = p * r + 8.333_452e-3;
    p = p * r + 4.166_579_6e-2;
    p = p * r + 1.666_666_5e-1;
    p = p * r + 0.5;
    let y = p * r * r + r + 1.0;
    y * f32::from_bits(((n + 127) as u32) << 23)
}

/// The Gaussian error linear unit, `x · Φ(x)`, in place. The error function
/// is Abramowitz and Stegun's 7.1.26, good to 1.5e-7.
pub fn gelu(values: &mut [f32]) {
    for value in values {
        let x = *value;
        let z = x.abs() * std::f32::consts::FRAC_1_SQRT_2;
        let t = 1.0 / (1.0 + 0.327_591_1 * z);
        let poly = t * (0.254_829_6 + t * (-0.284_496_72 + t * (1.421_413_8 + t * (-1.453_152_1 + t * 1.061_405_4))));
        let erf = 1.0 - poly * exp(-z * z);
        *value = 0.5 * x * (1.0 + erf.copysign(x));
    }
}

/// Softmax along each row of `rows` rows of `n`, in place.
pub fn softmax_rows(values: &mut [f32], n: usize) {
    for row in values.chunks_exact_mut(n) {
        let max = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let mut sum = 0.0;
        for v in row.iter_mut() {
            *v = exp(*v - max);
            sum += *v;
        }
        let scale = 1.0 / sum;
        for v in row.iter_mut() {
            *v *= scale;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_matches_the_library() {
        let mut x = -80.0_f32;
        while x < 80.0 {
            let (ours, theirs) = (exp(x), x.exp());
            assert!((ours - theirs).abs() <= theirs * 2e-6, "exp({x}) = {ours}, not {theirs}");
            x += 0.137;
        }
    }

    #[test]
    fn gelu_matches_its_definition() {
        // erf by its series, in f64.
        let erf = |x: f64| {
            let (mut term, mut sum) = (x, x);
            for n in 1..80 {
                term *= -x * x / f64::from(n);
                sum += term / f64::from(2 * n + 1);
            }
            sum * 2.0 / std::f64::consts::PI.sqrt()
        };
        let mut values: Vec<f32> = (-60..=60).map(|i| i as f32 / 10.0).collect();
        let expected: Vec<f64> = values.iter().map(|&x| 0.5 * f64::from(x) * (1.0 + erf(f64::from(x) / 2f64.sqrt()))).collect();
        gelu(&mut values);
        for (got, want) in values.iter().zip(expected) {
            assert!((f64::from(*got) - want).abs() < 2e-6, "{got} vs {want}");
        }
    }

    #[test]
    fn products_agree_with_the_definition() {
        let a: Vec<f32> = (0..6).map(|i| i as f32).collect(); // 2×3
        let b: Vec<f32> = (0..12).map(|i| (i as f32) * 0.5 - 1.0).collect();
        let mut c = vec![0.0; 8];
        mul_abt(2, 3, 4, &a, &b, &mut c); // b is 4×3
        for i in 0..2 {
            for j in 0..4 {
                let want: f32 = (0..3).map(|k| a[i * 3 + k] * b[j * 3 + k]).sum();
                assert!((c[i * 4 + j] - want).abs() < 1e-5);
            }
        }
        mul_ab(2, 3, 4, &a, &b, &mut c); // b is 3×4
        for i in 0..2 {
            for j in 0..4 {
                let want: f32 = (0..3).map(|k| a[i * 3 + k] * b[k * 4 + j]).sum();
                assert!((c[i * 4 + j] - want).abs() < 1e-5);
            }
        }
    }
}
