//! Steady-state processing must not allocate: after a short warm-up the
//! buffers have reached their high-water size.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use sonkore_soundtouch::{CHUNK_FRAMES, TimeStretch};

struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[test]
fn no_allocation_in_steady_state() {
    // (pitch, tempo, rate, quick seek, channels)
    let settings =
        [(3.0, 1.0, 1.0, false, 2), (-4.0, 1.3, 1.0, true, 2), (0.0, 0.7, 1.2, false, 1), (2.0, 1.0, 0.9, false, 6)];
    for (pitch, tempo, rate, quick, ch) in settings {
        let mut ts = TimeStretch::new(48000, ch);
        let c = ts.control().clone();
        c.set_enabled(true);
        c.set_pitch_semitones(pitch);
        c.set_tempo(tempo);
        c.set_rate(rate);
        c.set_use_quick_seek(quick);
        let mut x = 0x1234_5678u32;
        let block: Vec<f32> = (0..1024 * ch)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as i32 as f32 / 2147483648.0 * 0.5
            })
            .collect();
        let mut out = vec![0.0f32; CHUNK_FRAMES * ch];
        let mut run = |ts: &mut TimeStretch, blocks: usize| {
            for _ in 0..blocks {
                ts.apply_pending_changes();
                ts.put_samples(&block);
                while ts.receive_samples(&mut out) > 0 {}
            }
        };
        run(&mut ts, 100);
        let before = ALLOCS.load(Ordering::Relaxed);
        run(&mut ts, 1000);
        let after = ALLOCS.load(Ordering::Relaxed);
        assert_eq!(after - before, 0, "allocations at pitch {pitch} tempo {tempo} rate {rate} quick {quick} ch {ch}");
    }
}
