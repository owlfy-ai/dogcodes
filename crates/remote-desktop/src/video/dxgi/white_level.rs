//! Identify the original display's HDR reference white without using a default
//! monitor or guessing when mirrored targets have an ambiguous source name.
use super::Fault;
use windows::Win32::{
    Devices::Display::*,
    Foundation::{
        ERROR_ACCESS_DENIED, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, LUID, WIN32_ERROR,
    },
};

pub(super) struct Target(DISPLAYCONFIG_DEVICE_INFO_HEADER);

fn check(code: WIN32_ERROR) -> std::result::Result<(), Fault> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else if code == ERROR_ACCESS_DENIED {
        Err(Fault::Denied)
    } else {
        Err(Fault::Unavailable)
    }
}

impl Target {
    pub fn find(adapter: LUID, name: &[u16; 32]) -> std::result::Result<Self, Fault> {
        // Topology may change between buffer sizing and reading. Retry only the
        // documented size race; all allocations and retries remain bounded.
        for _ in 0..3 {
            let (mut path_count, mut mode_count) = (0, 0);
            // SAFETY: initialized size outputs; no buffers are passed yet.
            check(unsafe {
                GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
            })?;
            if path_count == 0 || path_count > 512 || mode_count == 0 || mode_count > 2048 {
                return Err(Fault::Unavailable);
            }
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
            // SAFETY: buffers have exactly the capacities supplied to the API;
            // both initialized counts are updated by Windows before iteration.
            let code = unsafe {
                QueryDisplayConfig(
                    QDC_ONLY_ACTIVE_PATHS,
                    &mut path_count,
                    paths.as_mut_ptr(),
                    &mut mode_count,
                    modes.as_mut_ptr(),
                    None,
                )
            };
            if code == ERROR_INSUFFICIENT_BUFFER {
                continue;
            }
            check(code)?;
            if path_count as usize > paths.len() || mode_count as usize > modes.len() {
                return Err(Fault::Unavailable);
            }
            let mut target = None;
            for path in &paths[..path_count as usize] {
                if path.sourceInfo.adapterId != adapter {
                    continue;
                }
                let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                    header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                        size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                        adapterId: path.sourceInfo.adapterId,
                        id: path.sourceInfo.id,
                    },
                    ..Default::default()
                };
                // SAFETY: header is the first repr(C) field of the complete
                // initialized request, with its exact size and source identity.
                let code = unsafe { DisplayConfigGetDeviceInfo(&mut source.header) };
                check(WIN32_ERROR(code as u32))?;
                let end = |v: &[u16]| v.iter().position(|c| *c == 0).unwrap_or(v.len());
                if source.viewGdiDeviceName[..end(&source.viewGdiDeviceName)] != name[..end(name)] {
                    continue;
                }
                if target.is_some() {
                    return Err(Fault::Unavailable);
                }
                target = Some(Self(DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
                    size: size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32,
                    adapterId: path.targetInfo.adapterId,
                    id: path.targetInfo.id,
                }));
            }
            return target.ok_or(Fault::Unavailable);
        }
        Err(Fault::Unavailable)
    }

    pub fn read(&self) -> std::result::Result<u32, Fault> {
        let mut white = DISPLAYCONFIG_SDR_WHITE_LEVEL {
            header: self.0,
            ..Default::default()
        };
        // SAFETY: complete initialized repr(C) request, exact size and the
        // original target identity; callers reject a stale DXGI factory first.
        let code = unsafe { DisplayConfigGetDeviceInfo(&mut white.header) };
        check(WIN32_ERROR(code as u32))?;
        if white.SDRWhiteLevel == 0 {
            return Err(Fault::Unavailable);
        }
        Ok(white.SDRWhiteLevel)
    }
}
