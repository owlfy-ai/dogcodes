//! A same-thread native loan with access to checked data/metadata descriptors.
use crate::Result;
use pipewire::{
    spa::{buffer::Data, sys},
    stream::Stream,
};
use std::ptr::NonNull;
pub(super) struct Loan<'a> {
    raw: NonNull<pipewire::sys::pw_buffer>,
    stream: &'a Stream,
}
impl<'a> Loan<'a> {
    pub fn dequeue(stream: &'a Stream) -> Option<Self> {
        // SAFETY: called only from this stream's PipeWire loop, returning one
        // exclusive loan whose Drop queues it before the stream is destroyed.
        NonNull::new(unsafe { stream.dequeue_raw_buffer() }).map(|raw| Self { raw, stream })
    }
    pub fn raw(&self) -> Result<&sys::spa_buffer> {
        // SAFETY: the dequeued pw_buffer stays live through this loan.
        unsafe { self.raw.as_ref().buffer.as_ref() }.ok_or_else(|| "Wayland 原生缓冲区缺失".into())
    }
    pub fn datas_mut(&mut self) -> Result<&mut [Data]> {
        let raw = self.raw()?;
        if raw.n_datas == 0 {
            return Ok(&mut []);
        }
        if raw.n_datas > 16 || raw.datas.is_null() {
            return Err("Wayland 数据描述符无效".into());
        }
        // SAFETY: the native buffer owns this n_datas array for the exclusive
        // loan; libspa Data is transparent over spa_data. No data borrow escapes
        // queueing, and count/null checks precede constructing the slice.
        Ok(unsafe {
            std::slice::from_raw_parts_mut(raw.datas.cast::<Data>(), raw.n_datas as usize)
        })
    }
}
impl Drop for Loan<'_> {
    fn drop(&mut self) {
        // SAFETY: this is exactly the original still-live loan, queued once on
        // its owning PipeWire thread. The stream borrow outlives this call.
        unsafe {
            self.stream.queue_raw_buffer(self.raw.as_ptr());
        }
    }
}
