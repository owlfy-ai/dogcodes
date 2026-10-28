//! Pack the existing tight OpenH264 I420 planes directly into an owned output
//! mapping: Y followed by interleaved UV, with no intermediate NV12 allocation.
use crate::Result;
use openh264::formats::{YUVBuffer, YUVSource};

pub(super) fn write(source: &YUVBuffer, destination: &mut [u8]) -> Result<()> {
    let (width, height) = source.dimensions();
    if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err("NV12 画面尺寸无效".into());
    }
    let pixels = width.checked_mul(height).ok_or("NV12 画面尺寸溢出")?;
    let length = pixels.checked_add(pixels / 2).ok_or("NV12 画面尺寸溢出")?;
    // Validate before modifying any byte. YUVBuffer itself guarantees tight
    // planes with exactly width*height and width*height/4 bytes.
    let destination = destination.get_mut(..length).ok_or("NV12 输入缓冲区不足")?;
    let (luma, chroma) = destination.split_at_mut(pixels);
    luma.copy_from_slice(source.y());
    for (out, (u, v)) in chroma
        .chunks_exact_mut(2)
        .zip(source.u().iter().zip(source.v()))
    {
        out.copy_from_slice(&[*u, *v]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_known_i420_as_nv12_with_uv_order_and_preserves_allocation_tail() {
        let source = YUVBuffer::from_vec(
            vec![
                16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 40, 41, 42, 43,
                200, 201, 202, 203,
            ],
            4,
            4,
        );
        let mut mapped = [0xee; 32];
        write(&source, &mut mapped).unwrap();
        assert_eq!(
            &mapped[..24],
            &[
                16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 40, 200, 41, 201,
                42, 202, 43, 203,
            ]
        );
        assert_eq!(&mapped[24..], &[0xee; 8]);
        assert_eq!(source.u(), &[40, 41, 42, 43]);
        assert_eq!(source.v(), &[200, 201, 202, 203]);
    }

    #[test]
    fn undersized_mapping_and_empty_source_fail_before_any_write() {
        let source = YUVBuffer::from_vec(vec![81, 82, 83, 84, 90, 240], 2, 2);
        let mut mapped = [0xee; 5];
        assert!(write(&source, &mut mapped).is_err());
        assert_eq!(mapped, [0xee; 5]);
        let empty = YUVBuffer::new(0, 0);
        assert!(write(&empty, &mut mapped).is_err());
        assert_eq!(mapped, [0xee; 5]);
        let mut complete = [0; 6];
        write(&source, &mut complete).unwrap();
        assert_eq!(complete, [81, 82, 83, 84, 90, 240]);
    }
}
