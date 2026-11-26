//! Immediate samplers: the console's four sampler words (the GPU's public
//! `SQ_IMG_SAMP_WORD0-3` layout) as a D3D12 static sampler.
//!
//! Word 0: address modes x, y, z (bits 0-8, 3 each), log2 of the maximum
//! anisotropy (9-11), comparison function (12-14), reduction (29-30: 0
//! filtering, 1 minimum, 2 maximum). Word 1: minimum and maximum LOD
//! (0-11, 12-23, unsigned 4.8). Word 2: LOD bias (0-13, signed 5.8),
//! magnification and minification filters (20-21, 22-23: point, linear,
//! anisotropic point, anisotropic linear), mip filter (26-27: none, point,
//! linear). Word 3: border colour (30-31: transparent black, opaque black,
//! opaque white, custom).
//!
//! Comparison is the sampling instruction's, not the sampler's: the console
//! encodes a comparison function in every sampler (D3D's default, less or
//! equal, when unused).
//!
//! Anisotropic point (2) in the magnification or minification field is
//! point sampling on that axis: bit 0 of each field is linear, and only
//! anisotropic linear (3) asks for anisotropy. A title (or the console's
//! driver, for a point filter with a maximum anisotropy set) writes 2 for
//! lookup tables, G-buffer fetches and dither or noise textures that must
//! not blend; D3D12 has no anisotropic point min/mag filter, so 2 becomes
//! plain point ([`anisotropic`]). The mip field (none, point, linear) has
//! no anisotropic value. `VICTORY_SAMPLER_ANISO_POINT=0` restores the
//! earlier decode, which took 2 as anisotropic linear ([`aniso_point_on`]).

/// Whether anisotropic point (2) in a sampler's magnification or
/// minification filter is point sampling (`VICTORY_SAMPLER_ANISO_POINT`; on
/// by default, `=0` takes it as anisotropic linear as before). Read for the
/// immediate samplers of translations and for the console's sampler
/// descriptions alike. Not in the shader cache's key: the host re-derives
/// each immediate sampler's filter from its words when it makes a root
/// signature ([`StaticSampler::refiltered`]), so a cached translation
/// follows the switch whichever decode it was cached with.
pub fn aniso_point_on() -> bool {
    #[cfg(test)]
    if let Some(on) = TEST_ANISO_POINT.get() {
        return on;
    }
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_SAMPLER_ANISO_POINT").is_none_or(|v| v != "0"))
}

#[cfg(test)]
thread_local! {
    /// A test's choice for [`aniso_point_on`], for its thread only.
    pub(super) static TEST_ANISO_POINT: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Whether a sampler with these magnification and minification filters
/// (0 point, 1 bilinear, 2 anisotropic point, 3 anisotropic bilinear) is an
/// anisotropic D3D12 sampler. With `aniso_point` (the fix): only when
/// neither axis is point (bit 0 clear: 0 or 2) and one is anisotropic
/// linear; D3D12's anisotropic filters are linear on both axes, so a point
/// axis never takes one. Without: whenever either is 2 or 3, as before.
pub fn anisotropic(mag: u32, min: u32, aniso_point: bool) -> bool {
    if aniso_point {
        let linear = |f: u32| f == 1 || f == 3;
        linear(mag) && linear(min) && (mag == 3 || min == 3)
    } else {
        mag >= 2 || min >= 2
    }
}

/// Whether the field is anisotropic point (2), for the diagnostics counts.
pub fn is_aniso_point(mag: u32, min: u32) -> bool {
    mag == 2 || min == 2
}

/// A static sampler in D3D12 terms; the integers are the `D3D12_FILTER`,
/// `D3D12_TEXTURE_ADDRESS_MODE`, `D3D12_COMPARISON_FUNC` and
/// `D3D12_STATIC_BORDER_COLOR` values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StaticSampler {
    pub filter: u32,
    /// U, V, W.
    pub address: [u32; 3],
    pub mip_lod_bias: f32,
    pub max_anisotropy: u32,
    pub comparison: u32,
    pub border: u32,
    pub min_lod: f32,
    pub max_lod: f32,
}

// The floats come from fixed-point fields: never NaN.
impl Eq for StaticSampler {}

impl StaticSampler {
    /// The filter supported by the host device. Anisotropic point-mip
    /// filtering is optional (`D3D12_OPTIONS19`); older devices retain the
    /// previous linear-mip approximation. Comparison/min/max reduction
    /// bits and the sampler's LOD bounds are unchanged.
    pub fn filter_for_host(&self, aniso_point_mip_supported: bool) -> u32 {
        if !aniso_point_mip_supported && self.filter & !0x180 == 0x54 {
            self.filter | 1
        } else {
            self.filter
        }
    }

    /// Decodes the four sampler words ([`aniso_point_on`] decides how
    /// anisotropic point is taken).
    pub fn from_words(w: [u32; 4]) -> Self {
        Self::from_words_as(w, aniso_point_on())
    }

    /// The magnification and minification filter fields of the words.
    pub fn mag_min(w: [u32; 4]) -> (u32, u32) {
        (w[2] >> 20 & 3, w[2] >> 22 & 3)
    }

    /// The D3D12 filter of the words, without the comparison bit (that is
    /// the sampling instruction's, added by the translation): anisotropic
    /// point taken as point with `aniso_point` ([`anisotropic`]).
    pub fn filter_of(w: [u32; 4], aniso_point: bool) -> u32 {
        let (mag, min) = Self::mag_min(w);
        let mip = w[2] >> 26 & 3;
        let reduction = match w[0] >> 29 & 3 {
            1 => 2,
            2 => 3,
            _ => 0,
        };
        if anisotropic(mag, min, aniso_point) {
            0x54 | u32::from(mip == 2) | reduction << 7
        } else {
            (min & 1) << 4 | (mag & 1) << 2 | u32::from(mip == 2) | reduction << 7
        }
    }

    /// This sampler with its filter decoded again from its words `w` as
    /// [`aniso_point_on`] says now, keeping the comparison bit (0x80) the
    /// translation gave it. The host does this when it makes a root
    /// signature, so translations cached under either decode give the same
    /// filter and the switch needs no place in the shader cache's key.
    pub fn refiltered(&self, w: [u32; 4]) -> Self {
        StaticSampler {
            filter: Self::filter_of(w, aniso_point_on()) | self.filter & 0x80,
            ..*self
        }
    }

    /// [`Self::from_words`] with the decode of anisotropic point given.
    pub fn from_words_as(w: [u32; 4], aniso_point: bool) -> Self {
        let bits = |word: u32, at: u32, n: u32| word >> at & ((1 << n) - 1);
        // wrap, mirror, clamp, mirror once; the half-border modes have no
        // D3D form and become their nearest.
        let address = |m: u32| [1, 2, 3, 5, 3, 5, 4, 5][m as usize];
        let mip = bits(w[2], 26, 2);
        let filter = Self::filter_of(w, aniso_point);
        let lod = |v: u32| v as f32 / 256.0;
        // Without a mip filter the base level is sampled.
        let (min_lod, max_lod) = if mip == 0 {
            (0.0, 0.0)
        } else {
            (lod(bits(w[1], 0, 12)), lod(bits(w[1], 12, 12)))
        };
        StaticSampler {
            filter,
            address: [0, 3, 6].map(|at| address(bits(w[0], at, 3))),
            mip_lod_bias: ((bits(w[2], 0, 14) << 18) as i32 >> 18) as f32 / 256.0,
            max_anisotropy: 1 << bits(w[0], 9, 3).min(4),
            comparison: bits(w[0], 12, 3) + 1,
            // shortcut: a custom border colour (type 3, a table entry) becomes
            // transparent black; upgrade when a title uses one.
            border: bits(w[3], 30, 2) % 3,
            min_lod,
            max_lod,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_compiled_samplers_decode() {
        // Words the console compiler wrote for static samplers of known
        // descriptions.
        let point = StaticSampler::from_words([0x3088, 0x0054_0100, 0x1400_0080, 0x8000_0000]);
        assert_eq!(
            point,
            StaticSampler {
                filter: 0,
                address: [1, 2, 3],
                mip_lod_bias: 0.5,
                max_anisotropy: 1,
                comparison: 4,
                border: 2,
                min_lod: 1.0,
                max_lod: 5.25,
            }
        );
        let aniso = StaticSampler::from_words([0x369E, 0x00FF_F000, 0x18F0_3E00, 0x8000_0000]);
        assert_eq!(
            (aniso.filter, aniso.address, aniso.max_anisotropy),
            (0x55, [4, 5, 3], 8)
        );
        assert_eq!((aniso.mip_lod_bias, aniso.max_lod), (-2.0, 4095.0 / 256.0));
        let compare = StaticSampler::from_words([0x3006, 0x00FF_F000, 0x1450_0000, 0x4000_0000]);
        assert_eq!(
            (compare.filter, compare.address, compare.border),
            (0x14, [4, 1, 1], 1)
        );
        let minimum =
            StaticSampler::from_words([0x2000_3003, 0x00FF_F000, 0x1850_0000, 0x8000_0000]);
        assert_eq!((minimum.filter, minimum.address[0]), (0x115, 5));
        let mag_mip = StaticSampler::from_words([0x3002, 0, 0x1810_0000, 0x8000_0000]);
        assert_eq!((mag_mip.filter, mag_mip.max_lod), (0x05, 0.0));
        let max_aniso =
            StaticSampler::from_words([0x4000_3800, 0x00FF_F000, 0x18F0_0F00, 0x8000_0000]);
        assert_eq!(
            (
                max_aniso.filter,
                max_aniso.max_anisotropy,
                max_aniso.mip_lod_bias
            ),
            (0x1D5, 16, 15.0)
        );
    }

    #[test]
    fn anisotropic_samplers_preserve_mip_point_filtering() {
        for (reduction, base) in [(0u32, 0x54u32), (1, 0x154), (2, 0x1D4)] {
            let w0 = reduction << 29;
            for mip in [0u32, 1] {
                let w2 = (mip << 26) | 0x00F0_0000; // mag = 3, min = 3
                let s = StaticSampler::from_words([w0, 0x00FF_F000, w2, 0]);
                assert_eq!(s.filter, base, "reduction={reduction} mip={mip}");
            }
            let w2 = (2 << 26) | 0x00F0_0000; // mip = 2 (linear)
            let s = StaticSampler::from_words([w0, 0x00FF_F000, w2, 0]);
            assert_eq!(s.filter, base | 1, "reduction={reduction} mip=2");
        }
        // Anisotropic point mag (2) with linear min (1) and point mip (1):
        // point magnification and linear minification, no anisotropy; the
        // earlier decode took it as anisotropic.
        let w = [0, 0, (1 << 26) | (1 << 22) | (2 << 20), 0];
        assert_eq!(StaticSampler::from_words_as(w, true).filter, 0x10);
        assert_eq!(StaticSampler::from_words_as(w, false).filter, 0x54);
    }

    /// Every magnification x minification pair (0 point, 1 bilinear, 2
    /// anisotropic point, 3 anisotropic bilinear), with each mip filter and
    /// reduction: point stays point on its axis and never makes the filter
    /// anisotropic; the earlier decode (switch off) is kept as it was.
    #[test]
    fn aniso_point_is_point_for_all_filter_pairs() {
        for mag in 0..4u32 {
            for min in 0..4u32 {
                for mip in 0..3u32 {
                    for (r, red) in [(0u32, 0u32), (1, 0x100), (2, 0x180)] {
                        let w = [r << 29 | 4 << 9, 0x00FF_F000, mip << 26 | min << 22 | mag << 20, 0];
                        let mip_bit = u32::from(mip == 2);
                        let point_or_linear = |f: u32| f & 1;
                        let fixed = if (mag, min) == (3, 3) || (mag, min) == (1, 3) || (mag, min) == (3, 1) {
                            0x54 | mip_bit
                        } else {
                            point_or_linear(min) << 4 | point_or_linear(mag) << 2 | mip_bit
                        } | red;
                        let old = if mag >= 2 || min >= 2 {
                            0x54 | mip_bit
                        } else {
                            min << 4 | mag << 2 | mip_bit
                        } | red;
                        let s = StaticSampler::from_words_as(w, true);
                        assert_eq!(s.filter, fixed, "fixed mag={mag} min={min} mip={mip} r={r}");
                        assert_eq!(StaticSampler::from_words_as(w, false).filter, old, "old mag={mag} min={min} mip={mip} r={r}");
                        // An anisotropic D3D12 filter only when neither axis is point.
                        if s.filter & 0x40 != 0 {
                            assert!(mag & 1 == 1 && min & 1 == 1, "mag={mag} min={min}");
                        }
                        // The fix changes the filter only where 2 appears or a
                        // point axis met anisotropic linear.
                        if !(mag == 2 || min == 2 || (mag == 0 && min == 3) || (mag == 3 && min == 0)) {
                            assert_eq!(fixed, old, "mag={mag} min={min}");
                        }
                        // A point axis comes out point on the host.
                        if mag & 1 == 0 {
                            assert_eq!(s.filter & 0x4, 0, "mag={mag} min={min}");
                        }
                        if min & 1 == 0 {
                            assert_eq!(s.filter & 0x10, 0, "mag={mag} min={min}");
                        }
                        // Maximum anisotropy and the LOD range are not the filter's.
                        assert_eq!(s.max_anisotropy, 16);
                        assert_eq!(s.max_lod, if mip == 0 { 0.0 } else { 4095.0 / 256.0 });
                    }
                }
            }
        }
        for mag in 0..4 {
            for min in 0..4 {
                assert_eq!(anisotropic(mag, min, true), mag & 1 == 1 && min & 1 == 1 && (mag == 3 || min == 3));
                assert_eq!(anisotropic(mag, min, false), mag >= 2 || min >= 2);
            }
        }
    }

    /// The host's re-decode from the words follows the switch and keeps the
    /// comparison bit the translation added.
    #[test]
    fn refiltered_follows_the_switch_and_keeps_comparison() {
        let w = [0, 0x00FF_F000, 2 << 26 | 2 << 22 | 2 << 20, 0];
        let was = TEST_ANISO_POINT.replace(Some(false));
        let mut cached = StaticSampler::from_words(w);
        assert_eq!(cached.filter, 0x55);
        cached.filter |= 0x80;
        TEST_ANISO_POINT.set(Some(true));
        assert_eq!(cached.refiltered(w).filter, 0x81, "point min and mag, linear mip, comparison");
        assert_eq!(StaticSampler::from_words(w).filter, 0x01);
        TEST_ANISO_POINT.set(Some(false));
        assert_eq!(cached.refiltered(w), cached);
        TEST_ANISO_POINT.set(was);
    }

    #[test]
    fn host_filter_falls_back_without_changing_reduction_or_lod() {
        for (filter, fallback) in [(0x54, 0x55), (0xD4, 0xD5), (0x154, 0x155), (0x1D4, 0x1D5)] {
            for mip in [0u32, 1] {
                let s = StaticSampler {
                    filter,
                    ..StaticSampler::from_words([0, 0x0054_0100, (mip << 26) | 0x00F0_0000, 0])
                };
                assert_eq!(s.filter_for_host(false), fallback);
                assert_eq!(s.filter_for_host(true), filter);
                assert_eq!(
                    (s.min_lod, s.max_lod),
                    if mip == 0 { (0.0, 0.0) } else { (1.0, 5.25) }
                );
            }
        }
    }

    #[test]
    fn host_filter_keeps_other_filters_unchanged() {
        for reduction in [0, 0x80, 0x100, 0x180] {
            for base in [0, 1, 4, 5, 0x10, 0x11, 0x14, 0x15, 0x55] {
                let s = StaticSampler {
                    filter: base | reduction,
                    ..StaticSampler::from_words([0; 4])
                };
                assert_eq!(s.filter_for_host(false), s.filter);
                assert_eq!(s.filter_for_host(true), s.filter);
            }
        }
    }

    #[test]
    fn mip_lod_bias_decodes_signed_14bit_fixed_point_extremes() {
        let cases = [
            (0x0000, 0.0_f32),
            (0x0001, 1.0 / 256.0),
            (0x1FFF, 8191.0 / 256.0),
            (0x2000, -32.0),
            (0x3FFF, -1.0 / 256.0),
        ];
        for (raw, expected) in cases {
            let s = StaticSampler::from_words([0, 0, raw, 0]);
            assert_eq!(s.mip_lod_bias, expected, "raw bias {raw:#x}");
        }
    }

    #[test]
    fn no_mip_filter_forces_zero_lod_range_and_non_zero_preserves_it() {
        let w1 = (0xABC << 12) | 0x123;
        let no_mip = StaticSampler::from_words([0, w1, 0, 0]);
        assert_eq!((no_mip.min_lod, no_mip.max_lod), (0.0, 0.0));

        let point_mip = StaticSampler::from_words([0, w1, 1 << 26, 0]);
        assert_eq!(
            (point_mip.min_lod, point_mip.max_lod),
            (0x123 as f32 / 256.0, 0xABC as f32 / 256.0)
        );
        assert_eq!(point_mip.filter, 0);

        let linear_mip = StaticSampler::from_words([0, w1, 2 << 26, 0]);
        assert_eq!(linear_mip.filter, 1);
    }
}
