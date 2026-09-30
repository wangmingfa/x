//! An owned, correctly aligned buffer for Win32 out-parameters.
//!
//! `Vec<u8>` has alignment 1. Handing its pointer to an API that fills a struct
//! is undefined behaviour the moment that struct needs more than a byte of
//! alignment, and the same applies when the filled bytes are read back through a
//! typed pointer. Every adapter here allocates through [`AlignedBuffer`]
//! instead, which is backed by `u64` and therefore satisfies the alignment of
//! every structure this crate touches.

use std::ffi::c_void;

/// `Vec<u8>` with the alignment a Win32 structure needs.
pub(crate) struct AlignedBuffer {
    words: Vec<u64>,
    len: usize,
}

impl AlignedBuffer {
    /// A zeroed buffer of at least `len` bytes.
    pub(crate) fn zeroed(len: usize) -> Self {
        let words = len.div_ceil(std::mem::size_of::<u64>()).max(1);
        Self {
            words: vec![0; words],
            len,
        }
    }

    /// Pointer to the first byte, suitably aligned for any struct used here.
    pub(crate) fn as_mut_ptr(&mut self) -> *mut c_void {
        self.words.as_mut_ptr().cast()
    }

    /// Const pointer to the first byte.
    pub(crate) fn as_ptr(&self) -> *const c_void {
        self.words.as_ptr().cast()
    }

    /// Number of usable bytes.
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Read one struct out of the buffer without assuming row alignment.
    ///
    /// Rows inside a Win32 table are packed at whatever offset the API chose, so
    /// an ordinary typed dereference would be an unaligned access.
    ///
    /// # Safety
    ///
    /// `T` must be the type the buffer was filled with, and the bytes at `start`
    /// must all belong to that `T`.
    pub(crate) unsafe fn read_at<T: Copy>(&self, start: usize) -> T {
        debug_assert!(start + std::mem::size_of::<T>() <= self.len);
        unsafe { std::ptr::read_unaligned(self.as_ptr().cast::<T>().add(start)) }
    }

    /// Read the first struct of the buffer.
    ///
    /// # Safety
    ///
    /// Same as [`AlignedBuffer::read_at`] with `start == 0`.
    pub(crate) unsafe fn read<T: Copy>(&self) -> T {
        unsafe { self.read_at::<T>(0) }
    }

    /// Borrow the bytes actually written.
    pub(crate) fn bytes(&self) -> &[u8] {
        // SAFETY: `u64` has no padding or invalid bit patterns.
        unsafe { std::slice::from_raw_parts(self.as_ptr().cast::<u8>(), self.len) }
    }

    /// Borrow the first four bytes as a little endian count.
    pub(crate) fn count(&self) -> u32 {
        self.bytes()
            .get(..4)
            .map(|raw| u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buffer_is_aligned_and_zeroed() {
        let buffer = AlignedBuffer::zeroed(37);
        assert_eq!(buffer.len(), 37);
        assert_eq!(buffer.as_ptr() as usize % std::mem::align_of::<u64>(), 0);
        assert!(buffer.bytes().iter().all(|byte| *byte == 0));
    }

    #[test]
    fn rows_can_start_at_any_offset() {
        let mut buffer = AlignedBuffer::zeroed(64);
        let base = buffer.as_mut_ptr().cast::<u8>();
        let value: u64 = 0x1122_3344_5566_7788;
        // SAFETY: both writes stay inside the allocation.
        unsafe {
            std::ptr::write_unaligned(base.cast::<u32>(), 1u32);
            // Offset 5 is not a multiple of the alignment of `u64`.
            std::ptr::write_unaligned(base.add(5).cast::<u64>(), value);
        }

        assert_eq!(buffer.count(), 1);
        // SAFETY: `value` was written at offset 5.
        assert_eq!(unsafe { buffer.read_at::<u64>(5) }, value);
    }
}
