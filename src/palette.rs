//! Palettes and curvature colouring, ported from flow.js.

pub struct Palette {
    pub name: &'static str,
    /// 0xRRGGBB, evenly spaced from t = 0 to t = 1.
    pub stops: &'static [u32],
}

#[rustfmt::skip]
pub const PALETTES: [Palette; 9] = [
    Palette { name: "Magma", stops: &[0x000004, 0x3b0f70, 0x8c2981, 0xde4968, 0xfe9f6d, 0xfcfdbf] },
    Palette { name: "Viridis", stops: &[0x440154, 0x3b528b, 0x21918c, 0x5ec962, 0xfde725] },
    Palette { name: "Ocean", stops: &[0x12305c, 0x1f5f99, 0x2e9cc4, 0x7fd3f8, 0xe6f9ff] },
    Palette { name: "Ember", stops: &[0x5a1300, 0xa8320a, 0xe0601a, 0xffaa45, 0xfff3c9] },
    Palette { name: "Aurora", stops: &[0x3a1c7a, 0x2353a8, 0x00a6a6, 0x7cfc9a, 0xf4ffb8] },
    Palette { name: "Rose quartz", stops: &[0x6b3a63, 0xa4567d, 0xd989a8, 0xf4c9d6, 0xfff6f8] },
    Palette { name: "Neon", stops: &[0xff007f, 0xa100ff, 0x2d6bff, 0x00e5ff] },
    Palette { name: "Gold", stops: &[0x5c3b00, 0x9c6b12, 0xd4a63a, 0xf5d77a, 0xfffbe6] },
    Palette { name: "Ink (mono)", stops: &[0x3a3a3a, 0xffffff] },
];

pub fn find(name: &str) -> Option<usize> {
    PALETTES
        .iter()
        .position(|p| p.name.eq_ignore_ascii_case(name))
}

pub fn rgb(hex: u32) -> [u8; 3] {
    [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8]
}

/// 256 colours with channels in 0..=255, stored as f32 like flow.js's Float32Array.
pub type Lut = [[f32; 3]; 256];

impl Palette {
    pub fn lut(&self, reverse: bool) -> Lut {
        let mut c: Vec<[f64; 3]> = self.stops.iter().map(|&h| rgb(h).map(f64::from)).collect();
        if reverse {
            c.reverse();
        }
        let mut lut = [[0.0; 3]; 256];
        for (i, out) in lut.iter_mut().enumerate() {
            let t = (i as f64 / 255.0) * (c.len() - 1) as f64;
            let j = (t.floor() as usize).min(c.len() - 2);
            let f = t - j as f64;
            for ch in 0..3 {
                out[ch] = (c[j][ch] * (1.0 - f) + c[j + 1][ch] * f) as f32;
            }
        }
        lut
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColourMode {
    /// Colour by |κ|: straight lines at one end of the palette, tight turns at the other.
    Magnitude,
    /// Colour by signed κ: straight lines mid-palette, left and right turns at the ends.
    Signed,
}

/// Palette index for a curvature, saturating around `kappa0`.
pub fn colour_index(kappa: f64, mode: ColourMode, kappa0: f64) -> u8 {
    let t = match mode {
        ColourMode::Signed => 0.5 + 0.5 * (kappa / kappa0).tanh(),
        ColourMode::Magnitude => (kappa.abs() / kappa0).tanh(),
    };
    // t ≥ 0, where Rust's round (half away from zero) agrees with Math.round (half up).
    (t * 255.0).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lut_ends_on_first_and_last_stops() {
        for p in &PALETTES {
            for reverse in [false, true] {
                let lut = p.lut(reverse);
                let (mut a, mut b) = (p.stops[0], p.stops[p.stops.len() - 1]);
                if reverse {
                    std::mem::swap(&mut a, &mut b);
                }
                assert_eq!(lut[0], rgb(a).map(f32::from), "{}", p.name);
                assert_eq!(lut[255], rgb(b).map(f32::from), "{}", p.name);
            }
        }
    }

    #[test]
    fn colour_index_covers_the_palette() {
        assert_eq!(colour_index(0.0, ColourMode::Magnitude, 1.5), 0);
        assert_eq!(colour_index(1e9, ColourMode::Magnitude, 1.5), 255);
        assert_eq!(colour_index(-1e9, ColourMode::Magnitude, 1.5), 255);
        assert_eq!(colour_index(0.0, ColourMode::Signed, 1.5), 128);
        assert_eq!(colour_index(-1e9, ColourMode::Signed, 1.5), 0);
        assert_eq!(colour_index(1e9, ColourMode::Signed, 1.5), 255);
    }

    #[test]
    fn find_by_name() {
        assert_eq!(find("magma"), Some(0));
        assert_eq!(find("Ink (mono)"), Some(8));
        assert_eq!(find("plasma"), None);
    }
}
