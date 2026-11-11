//! The network: a convolutional front end whose blocks attend across
//! frequency and then across time, six transformer layers, and a linear head
//! giving a beat and a downbeat logit for every frame.
//!
//! Activations are kept as `[frequency][time][channel]`, so the tokens one
//! attention reads across time are next to each other.

use crate::math::{gelu, mul_ab, mul_abt, softmax_rows};
use crate::weights::Weights;

/// Mel bands in a spectrogram frame.
pub const BANDS: usize = 128;
/// Width of every attention head.
const HEAD: usize = 32;
const EPSILON: f32 = 1e-5;

/// `x / |x| · √size · gamma` over each token.
fn rms_norm(x: &[f32], gamma: &[f32]) -> Vec<f32> {
    let dim = gamma.len();
    let scale = (dim as f32).sqrt();
    let mut out = vec![0.0; x.len()];
    for (token, normed) in x.chunks_exact(dim).zip(out.chunks_exact_mut(dim)) {
        let norm = token.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
        let by = scale / norm;
        for ((o, v), g) in normed.iter_mut().zip(token).zip(gamma) {
            *o = v * by * g;
        }
    }
    out
}

fn add_bias(x: &mut [f32], bias: &[f32]) {
    for token in x.chunks_exact_mut(bias.len()) {
        for (v, b) in token.iter_mut().zip(bias) {
            *v += b;
        }
    }
}

fn add(x: &mut [f32], y: &[f32]) {
    for (a, b) in x.iter_mut().zip(y) {
        *a += b;
    }
}

/// Rotary position embedding: each pair of a query's or key's values is
/// turned by an angle that grows with its position in the sequence.
struct Rotary {
    /// `[position][pair]`.
    cos: Vec<f32>,
    sin: Vec<f32>,
}

impl Rotary {
    fn new(freqs: &[f32], positions: usize) -> Self {
        assert_eq!(freqs.len(), HEAD / 2);
        let (mut cos, mut sin) = (Vec::with_capacity(positions * freqs.len()), Vec::with_capacity(positions * freqs.len()));
        for position in 0..positions {
            for freq in freqs {
                let angle = position as f32 * freq;
                cos.push(angle.cos());
                sin.push(angle.sin());
            }
        }
        Self { cos, sin }
    }

    #[inline]
    fn turn(&self, position: usize, from: &[f32], to: &mut [f32], scale: f32) {
        let at = position * (HEAD / 2);
        let (cos, sin) = (&self.cos[at..at + HEAD / 2], &self.sin[at..at + HEAD / 2]);
        for pair in 0..HEAD / 2 {
            let (a, b) = (from[2 * pair], from[2 * pair + 1]);
            to[2 * pair] = (a * cos[pair] - b * sin[pair]) * scale;
            to[2 * pair + 1] = (b * cos[pair] + a * sin[pair]) * scale;
        }
    }
}

/// Gated self-attention over sequences of tokens.
struct Attention {
    heads: usize,
    gamma: Vec<f32>,
    /// `[3 · heads · HEAD][dim]`: queries, then keys, then values.
    qkv: Vec<f32>,
    gates: Vec<f32>,
    gates_bias: Vec<f32>,
    out: Vec<f32>,
}

impl Attention {
    fn load(weights: &Weights, name: &str) -> Self {
        let gamma = weights.get(&format!("{name}.norm.gamma"));
        Self {
            heads: gamma.len() / HEAD,
            gamma,
            qkv: weights.get(&format!("{name}.to_qkv.weight")),
            gates: weights.get(&format!("{name}.to_gates.weight")),
            gates_bias: weights.get(&format!("{name}.to_gates.bias")),
            out: weights.get(&format!("{name}.to_out.0.weight")),
        }
    }

    /// `x` is `sequences` runs of `n` tokens; each run attends to itself.
    fn forward(&self, x: &[f32], sequences: usize, n: usize, rotary: &Rotary) -> Vec<f32> {
        let dim = self.gamma.len();
        let (heads, inner) = (self.heads, self.heads * HEAD);
        let tokens = sequences * n;
        let normed = rms_norm(x, &self.gamma);
        let mut qkv = vec![0.0; tokens * 3 * inner];
        mul_abt(tokens, dim, 3 * inner, &normed, &self.qkv, &mut qkv);
        let mut gates = vec![0.0; tokens * heads];
        mul_abt(tokens, dim, heads, &normed, &self.gates, &mut gates);
        add_bias(&mut gates, &self.gates_bias);
        for gate in &mut gates {
            *gate = 1.0 / (1.0 + crate::math::exp(-*gate));
        }

        let scale = (HEAD as f32).sqrt().recip();
        let (mut q, mut k, mut v) = (vec![0.0; n * HEAD], vec![0.0; n * HEAD], vec![0.0; n * HEAD]);
        let (mut scores, mut attended) = (vec![0.0; n * n], vec![0.0; n * HEAD]);
        let mut merged = vec![0.0; tokens * inner];
        for sequence in 0..sequences {
            for head in 0..heads {
                for i in 0..n {
                    let row = &qkv[(sequence * n + i) * 3 * inner..][..3 * inner];
                    let at = head * HEAD;
                    rotary.turn(i, &row[at..at + HEAD], &mut q[i * HEAD..][..HEAD], scale);
                    rotary.turn(i, &row[inner + at..][..HEAD], &mut k[i * HEAD..][..HEAD], 1.0);
                    v[i * HEAD..][..HEAD].copy_from_slice(&row[2 * inner + at..][..HEAD]);
                }
                mul_abt(n, HEAD, n, &q, &k, &mut scores);
                softmax_rows(&mut scores, n);
                mul_ab(n, n, HEAD, &scores, &v, &mut attended);
                for i in 0..n {
                    let gate = gates[(sequence * n + i) * heads + head];
                    let to = &mut merged[(sequence * n + i) * inner + head * HEAD..][..HEAD];
                    for (o, a) in to.iter_mut().zip(&attended[i * HEAD..][..HEAD]) {
                        *o = a * gate;
                    }
                }
            }
        }
        let mut out = vec![0.0; tokens * dim];
        mul_abt(tokens, inner, dim, &merged, &self.out, &mut out);
        out
    }
}

struct FeedForward {
    gamma: Vec<f32>,
    widen: Vec<f32>,
    widen_bias: Vec<f32>,
    narrow: Vec<f32>,
    narrow_bias: Vec<f32>,
}

impl FeedForward {
    fn load(weights: &Weights, name: &str) -> Self {
        Self {
            gamma: weights.get(&format!("{name}.net.0.gamma")),
            widen: weights.get(&format!("{name}.net.1.weight")),
            widen_bias: weights.get(&format!("{name}.net.1.bias")),
            narrow: weights.get(&format!("{name}.net.4.weight")),
            narrow_bias: weights.get(&format!("{name}.net.4.bias")),
        }
    }

    fn forward(&self, x: &[f32]) -> Vec<f32> {
        let (dim, wide) = (self.gamma.len(), self.widen_bias.len());
        let tokens = x.len() / dim;
        let normed = rms_norm(x, &self.gamma);
        let mut hidden = vec![0.0; tokens * wide];
        mul_abt(tokens, dim, wide, &normed, &self.widen, &mut hidden);
        add_bias(&mut hidden, &self.widen_bias);
        gelu(&mut hidden);
        let mut out = vec![0.0; tokens * dim];
        mul_abt(tokens, wide, dim, &hidden, &self.narrow, &mut out);
        add_bias(&mut out, &self.narrow_bias);
        out
    }
}

/// A convolution over `across` frequencies and three frames, stepping
/// `across` frequencies at a time, then batch norm and GELU.
struct Conv {
    across: usize,
    inputs: usize,
    outputs: usize,
    /// `[output][(frequency · 3 + frame) · inputs + input]`.
    kernel: Vec<f32>,
    scale: Vec<f32>,
    shift: Vec<f32>,
}

/// A batch norm at inference is a scale and a shift per channel.
fn batch_norm(weights: &Weights, name: &str) -> (Vec<f32>, Vec<f32>) {
    let (gain, bias) = (weights.get(&format!("{name}.weight")), weights.get(&format!("{name}.bias")));
    let (mean, variance) = (weights.get(&format!("{name}.running_mean")), weights.get(&format!("{name}.running_var")));
    let scale: Vec<f32> = gain.iter().zip(&variance).map(|(g, v)| g / (v + EPSILON).sqrt()).collect();
    let shift = bias.iter().zip(&mean).zip(&scale).map(|((b, m), s)| b - m * s).collect();
    (scale, shift)
}

impl Conv {
    fn load(weights: &Weights, conv: &str, norm: &str) -> Self {
        let tensor = weights.tensor(conv);
        let (outputs, inputs, across, frames) = (tensor.shape[0], tensor.shape[1], tensor.shape[2], tensor.shape[3]);
        assert_eq!(frames, 3);
        let mut kernel = vec![0.0; tensor.data.len()];
        for o in 0..outputs {
            for i in 0..inputs {
                for f in 0..across {
                    for t in 0..3 {
                        kernel[o * across * 3 * inputs + (f * 3 + t) * inputs + i] = tensor.data[((o * inputs + i) * across + f) * 3 + t];
                    }
                }
            }
        }
        let (scale, shift) = batch_norm(weights, norm);
        Self { across, inputs, outputs, kernel, scale, shift }
    }

    /// `x` is `[bands][frames][inputs]`; returns `[bands / across][frames][outputs]`.
    fn forward(&self, x: &[f32], bands: usize, frames: usize) -> Vec<f32> {
        let (inputs, across) = (self.inputs, self.across);
        let (rows, width) = (bands / across, across * 3 * inputs);
        let mut patches = vec![0.0; rows * frames * width];
        for row in 0..rows {
            for frame in 0..frames {
                let patch = &mut patches[(row * frames + frame) * width..][..width];
                for f in 0..across {
                    for t in 0..3 {
                        // One frame of zero padding either side.
                        let Some(source) = (frame + t).checked_sub(1).filter(|&s| s < frames) else { continue };
                        let from = ((row * across + f) * frames + source) * inputs;
                        patch[(f * 3 + t) * inputs..][..inputs].copy_from_slice(&x[from..from + inputs]);
                    }
                }
            }
        }
        let mut out = vec![0.0; rows * frames * self.outputs];
        mul_abt(rows * frames, width, self.outputs, &patches, &self.kernel, &mut out);
        for token in out.chunks_exact_mut(self.outputs) {
            for ((v, s), b) in token.iter_mut().zip(&self.scale).zip(&self.shift) {
                *v = *v * s + b;
            }
        }
        gelu(&mut out);
        out
    }
}

struct Block {
    across_frequency: (Attention, FeedForward),
    across_time: (Attention, FeedForward),
    conv: Conv,
}

/// `[a][b][channels]` to `[b][a][channels]`.
fn swap(x: &[f32], a: usize, b: usize, channels: usize) -> Vec<f32> {
    let mut out = vec![0.0; x.len()];
    for i in 0..a {
        for j in 0..b {
            out[(j * a + i) * channels..][..channels].copy_from_slice(&x[(i * b + j) * channels..][..channels]);
        }
    }
    out
}

pub struct Model {
    /// The batch norm over the spectrogram's bands.
    input_scale: Vec<f32>,
    input_shift: Vec<f32>,
    stem: Conv,
    blocks: Vec<Block>,
    project: Vec<f32>,
    project_bias: Vec<f32>,
    layers: Vec<(Attention, FeedForward)>,
    gamma: Vec<f32>,
    head: Vec<f32>,
    head_bias: Vec<f32>,
    freqs: Vec<f32>,
}

impl Model {
    pub fn load(weights: &Weights) -> Self {
        let (input_scale, input_shift) = batch_norm(weights, "frontend.stem.bn1d");
        assert_eq!(input_scale.len(), BANDS);
        let blocks = (0..3)
            .map(|i| {
                let name = format!("frontend.blocks.{i}");
                Block {
                    across_frequency: (
                        Attention::load(weights, &format!("{name}.partial.attnF")),
                        FeedForward::load(weights, &format!("{name}.partial.ffF")),
                    ),
                    across_time: (
                        Attention::load(weights, &format!("{name}.partial.attnT")),
                        FeedForward::load(weights, &format!("{name}.partial.ffT")),
                    ),
                    conv: Conv::load(weights, &format!("{name}.conv2d.weight"), &format!("{name}.norm")),
                }
            })
            .collect();
        let layers = (0..)
            .map(|i| format!("transformer_blocks.layers.{i}"))
            .take_while(|name| weights.has(&format!("{name}.0.norm.gamma")))
            .map(|name| (Attention::load(weights, &format!("{name}.0")), FeedForward::load(weights, &format!("{name}.1"))))
            .collect();
        Self {
            input_scale,
            input_shift,
            stem: Conv::load(weights, "frontend.stem.conv2d.weight", "frontend.stem.bn2d"),
            blocks,
            project: weights.get("frontend.linear.weight"),
            project_bias: weights.get("frontend.linear.bias"),
            layers,
            gamma: weights.get("transformer_blocks.norm.gamma"),
            head: weights.get("task_heads.beat_downbeat_lin.weight"),
            head_bias: weights.get("task_heads.beat_downbeat_lin.bias"),
            freqs: weights.get("rotary.freqs"),
        }
    }

    /// Beat and downbeat logits for each frame of `spect`, which is
    /// `[frames][BANDS]`. The network was trained on 30 seconds at a time;
    /// longer pieces go through [`crate::chunks`].
    pub fn forward(&self, spect: &[f32]) -> Vec<[f32; 2]> {
        let frames = spect.len() / BANDS;
        if frames == 0 {
            return Vec::new();
        }
        let rotary = Rotary::new(&self.freqs, frames.max(BANDS));

        let mut x = vec![0.0; BANDS * frames];
        for (frame, bands) in spect.chunks_exact(BANDS).enumerate() {
            for (band, value) in bands.iter().enumerate() {
                x[band * frames + frame] = value * self.input_scale[band] + self.input_shift[band];
            }
        }
        let mut x = self.stem.forward(&x, BANDS, frames);
        let (mut bands, mut channels) = (BANDS / self.stem.across, self.stem.outputs);
        for block in &self.blocks {
            let mut by_frame = swap(&x, bands, frames, channels);
            let attended = block.across_frequency.0.forward(&by_frame, frames, bands, &rotary);
            add(&mut by_frame, &attended);
            let fed = block.across_frequency.1.forward(&by_frame);
            add(&mut by_frame, &fed);
            x = swap(&by_frame, frames, bands, channels);
            let attended = block.across_time.0.forward(&x, bands, frames, &rotary);
            add(&mut x, &attended);
            let fed = block.across_time.1.forward(&x);
            add(&mut x, &fed);
            x = block.conv.forward(&x, bands, frames);
            bands /= block.conv.across;
            channels = block.conv.outputs;
        }

        // One token a frame: its channels, each across the bands left.
        let width = channels * bands;
        let mut tokens = vec![0.0; frames * width];
        for band in 0..bands {
            for frame in 0..frames {
                for channel in 0..channels {
                    tokens[frame * width + channel * bands + band] = x[(band * frames + frame) * channels + channel];
                }
            }
        }
        let dim = self.project_bias.len();
        let mut x = vec![0.0; frames * dim];
        mul_abt(frames, width, dim, &tokens, &self.project, &mut x);
        add_bias(&mut x, &self.project_bias);
        for (attention, feed_forward) in &self.layers {
            let attended = attention.forward(&x, 1, frames, &rotary);
            add(&mut x, &attended);
            let fed = feed_forward.forward(&x);
            add(&mut x, &fed);
        }
        let x = rms_norm(&x, &self.gamma);
        let mut out = vec![0.0; frames * 2];
        mul_abt(frames, dim, 2, &x, &self.head, &mut out);
        add_bias(&mut out, &self.head_bias);
        // A downbeat is a beat too, so the beat logit is the sum of both.
        out.chunks_exact(2).map(|pair| [pair[0] + pair[1], pair[1]]).collect()
    }
}
