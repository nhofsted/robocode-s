// SPDX-License-Identifier: EPL-2.0

//! Adaptive binary range coder (LZMA's carry-less design) and the integer binarization on top.
//!
//! Encoder and decoder share one code path through [`Io`]: the encoder codes the value it is
//! given and returns it, the decoder ignores it and returns what it read.

const PROB_BITS: u32 = 12;
const ONE: u16 = 1 << PROB_BITS;
const TOP: u32 = 1 << 24;

/// An adaptive bit probability. It adapts fast while young (count-based shift), then settles.
#[derive(Clone, Copy)]
pub struct Bit {
    p: u16, // probability of a 0, out of 4096
    n: u8,
}

impl Default for Bit {
    fn default() -> Self {
        Self { p: ONE / 2, n: 0 }
    }
}

impl Bit {
    fn update(&mut self, bit: bool) {
        let shift = match self.n {
            0 => 1,
            1 => 2,
            2..=4 => 3,
            5..=12 => 4,
            _ => 5,
        };
        self.n = self.n.saturating_add(1);
        if bit {
            self.p -= self.p >> shift;
        } else {
            self.p += (ONE - self.p) >> shift;
        }
    }
}

pub trait Io {
    fn bit(&mut self, m: &mut Bit, v: bool) -> bool;
    /// Attributes the following bits to a cost category (encoder statistics only).
    fn label(&mut self, _label: usize) {}
    /// True once a decoder has read well past its input (corrupt data).
    fn exhausted(&self) -> bool {
        false
    }
}

pub struct Encoder {
    low: u64,
    range: u32,
    cache: u8,
    cache_size: u64,
    out: Vec<u8>,
    label: usize,
    /// Information content per label, in bits.
    pub costs: Vec<f64>,
}

impl Encoder {
    pub fn new(labels: usize) -> Self {
        Self {
            low: 0,
            range: u32::MAX,
            cache: 0,
            cache_size: 1,
            out: Vec::new(),
            label: 0,
            costs: vec![0.0; labels],
        }
    }

    fn shift_low(&mut self) {
        if (self.low as u32) < 0xFF00_0000 || (self.low >> 32) != 0 {
            let carry = (self.low >> 32) as u8;
            let mut temp = self.cache;
            loop {
                self.out.push(temp.wrapping_add(carry));
                temp = 0xFF;
                self.cache_size -= 1;
                if self.cache_size == 0 {
                    break;
                }
            }
            self.cache = (self.low >> 24) as u8;
        }
        self.cache_size += 1;
        self.low = (self.low & 0x00FF_FFFF) << 8;
    }

    /// The first byte is always 0 and the decoder reads zeros past the end, so both are trimmed.
    pub fn finish(mut self) -> Vec<u8> {
        for _ in 0..5 {
            self.shift_low();
        }
        debug_assert_eq!(self.out[0], 0);
        self.out.remove(0);
        while self.out.last() == Some(&0) {
            self.out.pop();
        }
        self.out
    }
}

impl Io for Encoder {
    fn bit(&mut self, m: &mut Bit, v: bool) -> bool {
        let p0 = f64::from(m.p) / f64::from(ONE);
        self.costs[self.label] -= if v { 1.0 - p0 } else { p0 }.log2();
        let bound = (self.range >> PROB_BITS) * u32::from(m.p);
        if v {
            self.low += u64::from(bound);
            self.range -= bound;
        } else {
            self.range = bound;
        }
        m.update(v);
        while self.range < TOP {
            self.range <<= 8;
            self.shift_low();
        }
        v
    }

    fn label(&mut self, label: usize) {
        self.label = label;
    }
}

pub struct Decoder<'a> {
    input: &'a [u8],
    pos: usize,
    range: u32,
    code: u32,
}

impl<'a> Decoder<'a> {
    pub fn new(input: &'a [u8]) -> Self {
        let mut d = Self {
            input,
            pos: 0,
            range: u32::MAX,
            code: 0,
        };
        for _ in 0..4 {
            d.code = (d.code << 8) | u32::from(d.next());
        }
        d
    }

    fn next(&mut self) -> u8 {
        let b = self.input.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        b
    }

    /// True if the decoder read more than the trimmed padding allows: the input was corrupt.
    pub fn overrun(&self) -> bool {
        self.pos > self.input.len() + 8
    }
}

impl Io for Decoder<'_> {
    fn exhausted(&self) -> bool {
        self.overrun()
    }

    fn bit(&mut self, m: &mut Bit, _v: bool) -> bool {
        let bound = (self.range >> PROB_BITS) * u32::from(m.p);
        let v = if self.code < bound {
            self.range = bound;
            false
        } else {
            self.code -= bound;
            self.range -= bound;
            true
        };
        m.update(v);
        while self.range < TOP {
            self.range <<= 8;
            self.code = (self.code << 8) | u32::from(self.next());
        }
        v
    }
}

pub const CONTEXTS: usize = 4;
const MAX_EXP: usize = 40;

/// Signed integers: zero flag, sign, unary bit length, then the bits below the leading one.
/// Everything except the low mantissa bits is coded under one of [`CONTEXTS`] contexts.
pub struct IntModel {
    zero: [Bit; CONTEXTS],
    sign: [Bit; CONTEXTS],
    exp: [[Bit; MAX_EXP]; CONTEXTS],
    high: [[Bit; MAX_EXP]; CONTEXTS],
    low: [[Bit; MAX_EXP]; MAX_EXP],
}

impl Default for IntModel {
    fn default() -> Self {
        Self {
            zero: Default::default(),
            sign: Default::default(),
            exp: [[Bit::default(); MAX_EXP]; CONTEXTS],
            high: [[Bit::default(); MAX_EXP]; CONTEXTS],
            low: [[Bit::default(); MAX_EXP]; MAX_EXP],
        }
    }
}

impl IntModel {
    pub fn code<I: Io>(&mut self, io: &mut I, ctx: usize, v: i64) -> i64 {
        if io.bit(&mut self.zero[ctx], v == 0) {
            return 0;
        }
        let negative = io.bit(&mut self.sign[ctx], v < 0);
        let mag = v.unsigned_abs();
        let len = (64 - mag.leading_zeros()).saturating_sub(1) as usize; // bits below the leading one
        let mut k = 0;
        while k < MAX_EXP - 1 && io.bit(&mut self.exp[ctx][k], k < len) {
            k += 1;
        }
        let mut r: u64 = 1;
        for i in (0..k).rev() {
            let model = if i == k - 1 {
                &mut self.high[ctx][k]
            } else {
                &mut self.low[k][i]
            };
            r = (r << 1) | u64::from(io.bit(model, (mag >> i) & 1 == 1));
        }
        let r = r as i64;
        if negative { -r } else { r }
    }
}

/// A small symbol from a binary tree of adaptive bits (`bits` levels).
pub struct SymbolModel {
    bits: u32,
    tree: Vec<Bit>,
}

impl SymbolModel {
    pub fn new(bits: u32) -> Self {
        Self {
            bits,
            tree: vec![Bit::default(); 1 << bits],
        }
    }

    pub fn code<I: Io>(&mut self, io: &mut I, v: usize) -> usize {
        let mut node = 1;
        for i in (0..self.bits).rev() {
            let b = io.bit(&mut self.tree[node], (v >> i) & 1 == 1);
            node = (node << 1) | usize::from(b);
        }
        node - (1 << self.bits)
    }
}
