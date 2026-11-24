//! The ring between the worker and the device thread:
//! one writer, one reader, no locks, no allocation once made. Indices only
//! grow; the capacity is a power of two so a mask finds the slot.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

#[repr(align(64))]
struct Index(AtomicUsize);

pub struct Ring {
    data: Box<[UnsafeCell<i32>]>,
    mask: usize,
    write: Index, // written by the producer only
    read: Index,  // written by the consumer only
}

// One producer and one consumer touch disjoint slots, ordered by the
// acquire / release pairs on the indices.
unsafe impl Sync for Ring {}
unsafe impl Send for Ring {}

impl Ring {
    /// A ring for at least `samples` samples.
    pub fn new(samples: usize) -> Self {
        let capacity = samples.max(2).next_power_of_two();
        let data = (0..capacity).map(|_| UnsafeCell::new(0)).collect::<Vec<_>>().into_boxed_slice();
        Ring { data, mask: capacity - 1, write: Index(AtomicUsize::new(0)), read: Index(AtomicUsize::new(0)) }
    }

    pub fn capacity(&self) -> usize {
        self.mask + 1
    }

    /// Samples waiting to be read (consumer's view).
    pub fn available_to_read(&self) -> usize {
        let w = self.write.0.load(Ordering::Acquire);
        let r = self.read.0.load(Ordering::Relaxed);
        w.wrapping_sub(r)
    }

    /// Free room (producer's view).
    pub fn available_to_write(&self) -> usize {
        let w = self.write.0.load(Ordering::Relaxed);
        let r = self.read.0.load(Ordering::Acquire);
        self.capacity() - w.wrapping_sub(r)
    }

    /// Samples queued, from any thread (a snapshot).
    pub fn queued(&self) -> usize {
        let r = self.read.0.load(Ordering::Acquire);
        let w = self.write.0.load(Ordering::Acquire);
        w.wrapping_sub(r)
    }

    /// The slots as raw samples (UnsafeCell<i32> has i32's layout).
    fn slots(&self) -> *mut i32 {
        UnsafeCell::raw_get(self.data.as_ptr())
    }

    /// Producer: copies as much of `src` as fits; returns how much.
    pub fn write(&self, src: &[i32]) -> usize {
        let w = self.write.0.load(Ordering::Relaxed);
        let n = src.len().min(self.available_to_write());
        let at = w & self.mask;
        let first = n.min(self.capacity() - at);
        // SAFETY: slots between write and read + capacity belong to the
        // producer; the two runs (up to the end, then from the start) stay
        // inside the buffer and do not overlap `src`.
        unsafe {
            std::ptr::copy_nonoverlapping(src.as_ptr(), self.slots().add(at), first);
            std::ptr::copy_nonoverlapping(src.as_ptr().add(first), self.slots(), n - first);
        }
        self.write.0.store(w.wrapping_add(n), Ordering::Release);
        n
    }

    /// Consumer: fills `dst` with up to its length; returns how much.
    pub fn read(&self, dst: &mut [i32]) -> usize {
        let r = self.read.0.load(Ordering::Relaxed);
        let n = dst.len().min(self.available_to_read());
        let at = r & self.mask;
        let first = n.min(self.capacity() - at);
        // SAFETY: slots between read and write belong to the consumer; the
        // two runs stay inside the buffer and do not overlap `dst`.
        unsafe {
            std::ptr::copy_nonoverlapping(self.slots().add(at), dst.as_mut_ptr(), first);
            std::ptr::copy_nonoverlapping(self.slots(), dst.as_mut_ptr().add(first), n - first);
        }
        self.read.0.store(r.wrapping_add(n), Ordering::Release);
        n
    }

    /// Empties the ring. Only while neither side is running (the worker
    /// held, the device thread muted and not reading).
    pub fn clear(&self) {
        let w = self.write.0.load(Ordering::Acquire);
        self.read.0.store(w, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_around() {
        let ring = Ring::new(8);
        let mut out = [0; 8];
        for round in 0..100 {
            let data: Vec<i32> = (0..5).map(|i| round * 10 + i).collect();
            assert_eq!(ring.write(&data), 5);
            assert_eq!(ring.read(&mut out[..5]), 5);
            assert_eq!(&out[..5], &data[..]);
        }
        assert_eq!(ring.write(&[1; 20]), 8);
        assert_eq!(ring.available_to_write(), 0);
    }

    #[test]
    fn copies_across_the_end_at_every_offset() {
        let ring = Ring::new(16);
        let mut next = 0i32;
        let mut want = 0i32;
        for len in (1..=16).cycle().take(200) {
            let data: Vec<i32> = (next..next + len).collect();
            next += len;
            assert_eq!(ring.write(&data), len as usize);
            let mut out = vec![0; len as usize];
            assert_eq!(ring.read(&mut out), len as usize);
            assert_eq!(out, (want..want + len).collect::<Vec<_>>());
            want += len;
        }
    }
}
