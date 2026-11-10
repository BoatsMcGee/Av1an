//! Functions for parsing frames from encoder output.
//!
//! Some functions are optimized with SIMD, and need
//! runtime detection that the corresponding feature
//! set is available before calling them.

#[cfg(test)]
mod tests;

use std::{borrow::Cow, collections::HashSet};

use crate::models::encoder::EncoderBase;

// We can safely always ignore this prefix, as the second number will
// always be at some point after this prefix. See examples of aomenc
// output below to see why this is the case.
#[rustfmt::skip]
const AOM_VPX_IGNORED_PREFIX: &str =
  "Pass x/x frame    x/";
// Pass 1/1 frame    3/2       2131B    5997 us 500.25 fps [ETA  unknown]
//                     ^ relevant output starts at this character
// Pass 1/1 frame   84/83     81091B  132314 us 634.85 fps [ETA  unknown]
//                     ^
// Pass 1/1 frame  142/141   156465B  208875 us 679.83 fps [ETA  unknown]
//                     ^
// Pass 1/1 frame 4232/4231 5622510B 5518075 us 766.93 fps [ETA  unknown]
//                     ^
// Pass 1/1 frame 13380/13379 17860525B   16760 ms 798.31 fps [ETA  unknown]
//                      ^
// Pass 1/1 frame 102262/102261 136473850B  131502 ms 777.65 fps [ETA  unknown]
// 1272F                       ^
// Pass 1/1 frame 1022621/1022611 136473850B  131502 ms 777.65 fps [ETA
// unknown]    1272F                        ^
//
// As you can see, the relevant part of the output always starts past
// the length of the ignored prefix.

#[inline]
pub fn parse_aom_vpx_frames(s: &str) -> Option<u64> {
    // The numbers for aomenc/vpxenc are buffered/encoded frames, so we want the
    // second number (actual encoded frames)
    let first_digit_index = s
        .as_bytes()
        .get(AOM_VPX_IGNORED_PREFIX.len() - 1..)?
        .iter()
        .position(|&c| c == b'/')?;

    let first_space_index = s
        .get(AOM_VPX_IGNORED_PREFIX.len() + first_digit_index..)?
        .as_bytes()
        .iter()
        .position(|&c| c == b' ')?
        + first_digit_index;

    let digits = s.get(
        AOM_VPX_IGNORED_PREFIX.len() + first_digit_index
            ..AOM_VPX_IGNORED_PREFIX.len() + first_space_index,
    )?;
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// x86 SIMD implementation of parsing aomenc/vpxenc output using
/// SSSE3+SSE4.1, returning the number of frames processed, or `None`
/// if the input did not match.
///
/// This function also works for parsing vpxenc output, as its progress
/// printing is exactly the same.
///
/// # Safety
///
/// The CPU must support SSSE3 and SSE4.1.
#[inline]
#[target_feature(enable = "ssse3,sse4.1")]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub unsafe fn parse_aom_vpx_frames_sse41(s: &[u8]) -> Option<u64> {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;
    use std::mem::transmute;

    // This implementation matches the *second* number in the output. Ex:
    // Pass 1/1 frame  142/141   156465B  208875 us 679.83 fps [ETA  unknown]
    //                     ^^^
    //                     matches this number and returns `Some(141)`
    //
    // Pass 1/1 frame 102262/102261 136473850B  131502 ms 777.65 fps [ETA
    // unknown] 1272F                       ^^^^^^
    //                       matches this number and returns `Some(102261)`
    //
    // Input is immutable: every load must fit the slice, not spare allocation
    // capacity. Short lines and counts outside the SIMD window use scalar parsing.
    let scalar = || parse_aom_vpx_frames(std::str::from_utf8(s).ok()?);

    // Number of bytes processed (size in bytes of xmm register)
    //
    // There is no benefit to using wider SIMD lanes in this case, so we just
    // use the most commonly available SIMD width. This is because we want
    // to parse the fewest number of bytes possible to get the correct result.
    const CHUNK_SIZE: usize = 16;

    let prefix = AOM_VPX_IGNORED_PREFIX.len();
    let Some(output_bytes) = s.get(prefix..prefix + CHUNK_SIZE) else {
        return scalar();
    };

    // Since the aomenc output follows a particular pattern, we can calculate
    // the position of the '/' character from the index of the first space
    // (how to do so is explained later on). We create this mask to find the
    // first space in the output.
    let spaces = _mm_set1_epi8(b' ' as i8);

    // SAFETY: output_bytes is a checked, readable 16-byte range inside s. The
    // caller guarantees SSSE3+SSE4.1, as required by this target-feature function.
    let relevant_output = unsafe { _mm_loadu_si128(output_bytes.as_ptr().cast()) };

    // Compare the relevant output to spaces to create a mask where each bit
    // is set to 1 if the corresponding character was a space, and 0 otherwise.
    // The LSB corresponds to the match between the first characters.
    //
    // Only the lower 16 bits are relevant, as the rest are always set to 0.
    let mask16 = _mm_movemask_epi8(_mm_cmpeq_epi8(relevant_output, spaces));
    if mask16 == 0 {
        return scalar();
    }

    // The bits in the mask are set as so:
    //
    //       "141   156465B  208875 us 679.83 fps [ETA  unknown]"
    // mask:  110000000111000
    //                    ^^^
    //                    These bits correspond to the first 3 characters:
    // "141".                    Since they do not match the space, they are
    // set to 0 in the mask.                    As printed, the leftmost bit
    // is the most significant bit.                 ^^^
    //                 These bits correspond to the 3 spaces after the "141".
    //
    //       "2/102261 136473850B  131502 ms 777.65 fps [ETA  unknown]    1272F"
    // mask:  100000000
    //         ^^^^^^^^
    //         These bits correspond to the first 8 characters: "2/102261".
    //
    // To get the index of the first space, we need to get the trailing zeros,
    // which correspond to the first characters.
    //
    // This value is such that `relevant_output[first_space_index]` gives the
    // actual first space character.
    let first_space_index = mask16.trailing_zeros() as usize;

    let slash_bytes = s.get(prefix - 1..prefix - 1 + CHUNK_SIZE)?;
    // SAFETY: slash_bytes is another checked 16-byte range inside s; the same
    // caller-guaranteed CPU feature precondition applies.
    let slash_output = unsafe { _mm_loadu_si128(slash_bytes.as_ptr().cast()) };
    let slash_mask = _mm_movemask_epi8(_mm_cmpeq_epi8(slash_output, _mm_set1_epi8(b'/' as i8)));
    if slash_mask == 0 {
        return scalar();
    }
    let first_digit_index = slash_mask.trailing_zeros() as usize;

    let ascending = _mm_set_epi8(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15);
    // The integer kernel handles at most 16 decimal digits (10^16-1 fits u64).
    // A missing/earlier space can also mean the actual count is beyond this window.
    let Some(num_digits @ 1..=CHUNK_SIZE) = first_space_index.checked_sub(first_digit_index) else {
        return scalar();
    };
    let end = prefix + first_space_index;
    let digits = s.get(prefix + first_digit_index..end)?;
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }

    let dynamic_mask = _mm_cmplt_epi8(ascending, _mm_set1_epi8(num_digits as i8));

    // At this point, we have done all the setup and can use the actual SIMD
    // integer parsing algorithm. The description of the algorithm can be
    // found here: https://kholdstare.github.io/technical/2020/05/26/faster-integer-parsing.html
    let chunk_bytes = s.get(end.checked_sub(CHUNK_SIZE)?..end)?;
    // SAFETY: The complete final 16-byte load range, not only its start, was
    // checked inside s. CPU support is guaranteed by the caller. Only the
    // validated ASCII digit span contributes after dynamic_mask is applied.
    let mut chunk = unsafe { _mm_loadu_si128(chunk_bytes.as_ptr().cast()) };

    let zeros = _mm_set1_epi8(b'0' as i8);
    chunk = _mm_sub_epi8(chunk, zeros);

    // Mask out the irrelevant bits, effectively parsing them as if they were 0.
    chunk = _mm_and_si128(chunk, dynamic_mask);

    let mult = _mm_set_epi8(1, 10, 1, 10, 1, 10, 1, 10, 1, 10, 1, 10, 1, 10, 1, 10);
    chunk = _mm_maddubs_epi16(chunk, mult);
    let mult = _mm_set_epi16(1, 100, 1, 100, 1, 100, 1, 100);
    chunk = _mm_madd_epi16(chunk, mult);
    chunk = _mm_packus_epi32(chunk, chunk);
    let mult = _mm_set_epi16(0, 0, 0, 0, 1, 10000, 1, 10000);
    chunk = _mm_madd_epi16(chunk, mult);

    // SAFETY: Both types occupy 128 bits and every bit pattern is valid for u64.
    let chunk = unsafe { transmute::<__m128i, [u64; 2]>(chunk) };

    Some(((chunk[0] & 0xffff_ffff) * 100_000_000) + (chunk[0] >> 32))
}

#[inline]
pub fn parse_rav1e_frames(s: &str) -> Option<u64> {
    #[rustfmt::skip]
  const RAV1E_IGNORED_PREFIX: &str =
    "encoded ";
    // encoded 1 frames, 126.416 fps, 16.32 Kb/s, elap. time: 1m 36s
    // encoded 12 frames, 126.416 fps, 16.32 Kb/s, elap. time: 1m 36s
    // encoded 122 frames, 126.416 fps, 16.32 Kb/s, elap. time: 1m 36s
    // encoded 1220 frames, 126.416 fps, 16.32 Kb/s, elap. time: 1m 36s
    // encoded 12207 frames, 126.416 fps, 16.32 Kb/s, elap. time: 1m 36s
    // encoded 12/240 frames, 126.416 fps, 16.32 Kb/s, elap. time: 1m 36s

    if !s.starts_with(RAV1E_IGNORED_PREFIX) {
        return None;
    }

    s.get(RAV1E_IGNORED_PREFIX.len()..)?
        .split_ascii_whitespace()
        .next()
        .map(|val| val.split_once('/').map_or(val, |(val, _)| val))
        .and_then(|s| s.parse().ok())
}

#[inline]
pub fn parse_svt_av1_frames(input: &str) -> Option<u64> {
    const SVT_AV1_LEGACY_PREFIX: &str = "Encoding frame";
    const SVT_AV1_NEW_PREFIX: &str = "Encoding:";

    let stripped = strip_ansi_escape_sequences(input);
    let s = stripped.as_ref();

    // Handle the new progress format introduced in https://gitlab.com/AOMediaCodec/SVT-AV1/-/merge_requests/2511
    if s.starts_with(SVT_AV1_NEW_PREFIX) {
        let line = s.get(SVT_AV1_NEW_PREFIX.len()..)?.trim_start();
        if let Some((frames, _)) = line.split_once('/')
            && let Ok(frames) = frames.parse()
        {
            // Frame count is known
            return Some(frames);
        }

        // Frame count unknown
        return line.split_ascii_whitespace().next().and_then(|s| s.parse().ok());
    }

    // Old progress format
    if s.starts_with(SVT_AV1_LEGACY_PREFIX) {
        return s
            .get(SVT_AV1_LEGACY_PREFIX.len()..)?
            .split_ascii_whitespace()
            .next()
            .and_then(|s| s.parse().ok());
    }

    // Unrecognized progress format
    None
}

#[inline]
pub fn parse_x26x_frames(s: &str) -> Option<u64> {
    s.split_ascii_whitespace()
        .find(|part| !part.starts_with('['))
        .map(|val| val.split_once('/').map_or(val, |(val, _)| val))
        .and_then(|s| s.parse().ok())
}

#[inline]
pub fn parse_vvc_frames(s: &str) -> Option<u64> {
    // Known totals add a percentage before frame=N/total; fps is not a frame count.
    let mut fields = s.strip_prefix("vvenc [info]: stats:")?.split_ascii_whitespace();
    let value = fields.find_map(|field| field.strip_prefix("frame="))?;
    let value = if value.is_empty() {
        fields.next()?
    } else {
        value
    };
    if !value.bytes().all(|byte| byte.is_ascii_digit() || byte == b'/') {
        return None;
    }
    let frame = if let Some((frame, total)) = value.split_once('/') {
        total.parse::<u64>().ok()?;
        frame
    } else {
        value
    };
    frame.parse().ok()
}

#[inline]
pub fn parse_ffmpeg_frames(s: &str) -> Option<u64> {
    // frame=  332 fps= 58 q=18.1 size=     512KiB time=00:00:13.76 bitrate=
    // 304.7kbits/s speed=2.42x elapsed=0:00:05.69
    const FFMPEG_PREFIX: &str = "frame=";
    if !s.starts_with(FFMPEG_PREFIX) {
        return None;
    }

    s.split_ascii_whitespace().nth(1).and_then(|s| s.parse().ok())
}

fn strip_ansi_escape_sequences(input: &str) -> Cow<'_, str> {
    const ESC: char = '\x1b';

    if !input.contains(ESC) {
        return Cow::Borrowed(input);
    }

    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == ESC {
            if matches!(chars.peek(), Some('[')) {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
                continue;
            }

            // Skip bare escape characters as well.
            continue;
        }

        output.push(ch);
    }

    Cow::Owned(output)
}

/// Returns the set of valid parameters given a help text for the given encoder
#[must_use]
#[inline]
pub fn valid_params(help_text: &str, encoder: EncoderBase) -> HashSet<Cow<'_, str>> {
    // x265 has 292 parameters, which is the most of any encoder, so we round up
    // slightly just in case
    let mut params = HashSet::with_capacity(300);

    for s in help_text.split_ascii_whitespace() {
        if s.starts_with('-') {
            if s.len() == 1 || s == "--" {
                continue;
            }

            if encoder == EncoderBase::X265 {
                // x265 does this: -m/--subme
                //        or even: -w/--[no-]weightp
                // So we need to ensure that in this case the short parameter is
                // also handled.
                let s = s.get("-x/".len()..).map_or(s, |stripped| {
                    if stripped.starts_with("--") {
                        #[expect(
                            clippy::string_slice,
                            reason = "we know the first two chars are '--'"
                        )]
                        params.insert(Cow::Borrowed(&s[..2]));

                        stripped
                    } else {
                        s
                    }
                });

                // Somehow x265 manages to have a buggy --help, where a single
                // option (--[no-]-hrd-concat) has an extra
                // dash.
                let arg = s
                    .strip_prefix("--[no-]")
                    .map(|stripped| stripped.strip_prefix('-').unwrap_or(stripped));

                if let Some(arg) = arg {
                    params.insert(Cow::Owned(format!("--{arg}")));
                    params.insert(Cow::Owned(format!("--no-{arg}")));
                    continue;
                }
            }

            // aomenc outputs '--tune=<arg>' for example, so we have to find the
            // character from the left so as to not miss the
            // leftmost char
            if let Some(idx) = s.find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
            {
                // In some weird cases (like with x264) there may be a dash
                // followed by a non alphanumeric character, so
                // we just ignore that.
                if idx > 1 {
                    params.insert(Cow::Owned(s.chars().take(idx).collect()));
                }
            } else {
                // It's a little concerning how *two* encoders manage to have
                // buggy help output.
                let arg = if encoder == EncoderBase::VPX {
                    // vpxenc randomly truncates the "Vizier Rate Control
                    // Options" in the help output, which
                    // sometimes causes it to truncate at a dash, which breaks
                    // the tests if we don't do this. Not
                    // sure what the correct solution in this case
                    // is.
                    s.strip_suffix('-').unwrap_or(s)
                } else {
                    s
                };

                params.insert(Cow::Borrowed(arg));
            }
        }
    }

    params
}
