//! Canonical Annex-B access units and cached parameter sets for native codecs.
use super::{EncodedFrame, Result, VideoProfile, validate_profile};

const MAX_FRAME: usize = 8 * 1024 * 1024;
#[derive(Default)]
pub(super) struct Headers {
    sps: Vec<u8>,
    pps: Vec<u8>,
}
fn nals(bytes: &[u8]) -> Result<Vec<&[u8]>> {
    if bytes.is_empty() || bytes.len() > MAX_FRAME {
        return Err("系统编码画面大小无效".into());
    }
    fn prefix(b: &[u8]) -> usize {
        if b.starts_with(&[0, 0, 0, 1]) {
            4
        } else if b.starts_with(&[0, 0, 1]) {
            3
        } else {
            0
        }
    }
    let mut out = vec![];
    if prefix(bytes) != 0 {
        let mut start = prefix(bytes);
        let mut i = start;
        while i < bytes.len() {
            let count = prefix(&bytes[i..]);
            if count > 0 {
                if start == i {
                    return Err("系统编码 NAL 为空".into());
                }
                if out.len() >= 8192 {
                    return Err("系统编码 NAL 数量超限".into());
                }
                out.push(&bytes[start..i]);
                start = i + count;
                i = start;
            } else {
                i += 1;
            }
        }
        if start == bytes.len() {
            return Err("系统编码 NAL 截断".into());
        }
        if out.len() >= 8192 {
            return Err("系统编码 NAL 数量超限".into());
        }
        out.push(&bytes[start..]);
    } else {
        let mut rest = bytes;
        while !rest.is_empty() {
            if rest.len() < 4 {
                return Err("系统编码长度截断".into());
            }
            let size = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
            rest = &rest[4..];
            if size == 0 || size > rest.len() {
                return Err("系统编码长度无效".into());
            }
            if out.len() >= 8192 {
                return Err("系统编码 NAL 数量超限".into());
            }
            out.push(&rest[..size]);
            rest = &rest[size..];
        }
    }
    if out.len() > 8192
        || out
            .iter()
            .any(|nal| nal.is_empty() || nal[0] & 0x80 != 0 || nal[0] & 31 == 0 || nal[0] & 31 > 23)
    {
        return Err("系统编码 NAL 无效".into());
    }
    Ok(out)
}
impl Headers {
    pub fn observe(&mut self, bytes: &[u8]) -> Result<()> {
        for nal in nals(bytes)? {
            if matches!(nal[0] & 31, 7 | 8) {
                if nal.len() > 65536 {
                    return Err("系统编码参数过长".into());
                }
                if nal[0] & 31 == 7 {
                    self.sps = nal.to_vec();
                } else {
                    self.pps = nal.to_vec();
                }
            }
        }
        Ok(())
    }
    pub fn frame(
        &mut self,
        bytes: &[u8],
        profile: VideoProfile,
        captured: std::time::Instant,
    ) -> Result<EncodedFrame> {
        self.observe(bytes)?;
        let nals = nals(bytes)?;
        let keyframe = nals.iter().any(|nal| nal[0] & 31 == 5);
        if !nals.iter().any(|nal| matches!(nal[0] & 31, 1..=5)) {
            return Err("系统编码没有返回画面".into());
        }
        if keyframe && (self.sps.is_empty() || self.pps.is_empty()) {
            return Err("系统关键帧缺少 SPS/PPS".into());
        }
        let mut data = Vec::with_capacity(bytes.len() + self.sps.len() + self.pps.len() + 16);
        let mut append = |nal: &[u8]| {
            data.extend_from_slice(&[0, 0, 0, 1]);
            data.extend_from_slice(nal);
        };
        for nal in nals.iter().filter(|nal| nal[0] & 31 == 9) {
            append(nal);
        }
        if keyframe {
            append(&self.sps);
            append(&self.pps);
        }
        for nal in nals
            .iter()
            .filter(|nal| nal[0] & 31 != 9 && (!keyframe || !matches!(nal[0] & 31, 7 | 8)))
        {
            append(nal);
        }
        if data.len() > MAX_FRAME {
            return Err("系统编码画面超过传输预算".into());
        }
        let frame = EncodedFrame {
            cursor_embedded: None,
            captured,
            data,
            keyframe,
            latency: captured.elapsed(),
        };
        validate_profile(&frame, profile)?;
        Ok(frame)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn caches_three_byte_headers_and_adds_them_to_each_actual_idr_in_canonical_form() {
        let mut h = Headers::default();
        h.observe(&[0, 0, 1, 0x67, 66, 0xc0, 31, 1, 0, 0, 1, 0x68, 1])
            .unwrap();
        let frame = h
            .frame(
                &[0, 0, 1, 0x09, 0xf0, 0, 0, 1, 0x65, 1],
                VideoProfile::ConstrainedBaseline31,
                std::time::Instant::now(),
            )
            .unwrap();
        assert!(frame.keyframe);
        assert_eq!(
            frame.data,
            [
                0, 0, 0, 1, 0x09, 0xf0, 0, 0, 0, 1, 0x67, 66, 0xc0, 31, 1, 0, 0, 0, 1, 0x68, 1, 0,
                0, 0, 1, 0x65, 1
            ]
        );
        assert!(
            !h.frame(
                &[0, 0, 0, 2, 0x41, 1],
                VideoProfile::ConstrainedBaseline31,
                std::time::Instant::now()
            )
            .unwrap()
            .keyframe
        );
    }
    #[test]
    fn incomplete_headers_bad_lengths_and_unnegotiated_profiles_are_rejected() {
        for bytes in [
            &[0, 0, 0, 1, 0x65, 1][..],
            &[0, 0, 0, 5, 0x41, 1],
            &[0, 0, 1],
            &[0, 0, 1, 0x80],
        ] {
            assert!(
                Headers::default()
                    .frame(
                        bytes,
                        VideoProfile::ConstrainedBaseline31,
                        std::time::Instant::now()
                    )
                    .is_err()
            );
        }
        let mut h = Headers::default();
        h.observe(&[0, 0, 1, 0x67, 100, 0, 51, 0, 0, 1, 0x68, 1])
            .unwrap();
        assert!(
            h.frame(
                &[0, 0, 1, 0x65, 1],
                VideoProfile::ConstrainedBaseline31,
                std::time::Instant::now()
            )
            .is_err()
        );
    }
}
