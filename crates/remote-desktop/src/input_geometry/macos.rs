use super::*;
#[derive(Default)]
pub(crate) struct Resolver;
pub(crate) struct Context;
impl Context {
    pub fn enter() -> Result<Self> {
        Ok(Self)
    }
}
impl Resolver {
    pub fn new() -> Self {
        Self
    }
    pub fn move_to(&self, input: &mut enigo::Enigo, x: i32, y: i32) -> Result<()> {
        use enigo::Mouse;
        input
            .move_mouse(x, y, enigo::Coordinate::Abs)
            .map_err(crate::native)
    }
    pub fn current(&self, display: &Display) -> Result<Display> {
        use objc2_core_graphics::{CGDisplayBounds, CGDisplayIsActive};
        if !CGDisplayIsActive(display.id) {
            return Err("显示器已断开".into());
        }
        let r = CGDisplayBounds(display.id);
        let values = [r.origin.x, r.origin.y, r.size.width, r.size.height];
        if values.iter().any(|v| !v.is_finite())
            || r.origin.x < f64::from(i32::MIN)
            || r.origin.y < f64::from(i32::MIN)
            || r.origin.x > f64::from(i32::MAX)
            || r.origin.y > f64::from(i32::MAX)
            || r.size.width < 1.0
            || r.size.height < 1.0
            || r.size.width > f64::from(u32::MAX)
            || r.size.height > f64::from(u32::MAX)
        {
            return Err("显示器当前坐标范围无效".into());
        }
        Bounds {
            x: r.origin.x as i32,
            y: r.origin.y as i32,
            width: r.size.width as u32,
            height: r.size.height as u32,
        }
        .apply(display)
    }
}
