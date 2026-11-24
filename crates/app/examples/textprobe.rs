//! Where Slint puts a vertically centred text: "H" in Inter, in a box at
//! y = 10 of height H, per scale; prints the ink rows (device pixels).
//!   cargo run -p sonkore --example textprobe
use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, PhysicalSize};
use std::rc::Rc;

slint::slint! {
    #[edition = "1.19"]
    import "../../../fonts/inter/400.ttf";
    import "../../../fonts/inter/600.ttf";
    import "../../../fonts/inter/700.ttf";
    export component Probe inherits Window {
        in property <length> px: 13px;
        in property <length> box: 16px;
        in property <int> weight: 400;
        in property <length> top: 10px;
        background: black;
        width: 100px;
        height: 70px;
        Text {
            x: 0; y: root.top; width: 100px; height: root.box;
            text: "H"; font-family: "Inter"; font-size: root.px; font-weight: root.weight; color: white;
            vertical-alignment: center;
        }
    }
}

struct P(Rc<MinimalSoftwareWindow>);
impl Platform for P {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn main() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(P(window.clone()))).unwrap();
    let probe = Probe::new().unwrap();
    probe.show().unwrap();
    for wt in [400, 600, 700] {
        probe.set_weight(wt);
        for scale in [1.0f32, 1.25, 1.5, 1.75, 2.0] {
            for px in [10.0f32, 11.0, 12.0, 13.0, 14.0, 16.0] {
                for top in [10.0f32, 11.0] {
                    probe.set_top(top);
                    for h in [16.0f32, 17.0, 18.0, 30.0, 33.0] {
                        probe.set_px(px);
                        probe.set_box(h);
                        window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: scale });
                        let (w, hh) = ((100.0 * scale).ceil() as u32, (70.0 * scale).ceil() as u32);
                        window.set_size(PhysicalSize::new(w, hh));
                        let mut buf = vec![PremultipliedRgbaColor::default(); (w * hh) as usize];
                        window.request_redraw();
                        window.draw_if_needed(|r| {
                            r.render(&mut buf, w as usize);
                        });
                        let (mut ink_top, mut bot) = (-1i32, -1i32);
                        for y in 0..hh {
                            for x in 0..w {
                                if buf[(y * w + x) as usize].red > 128 {
                                    if ink_top < 0 {
                                        ink_top = y as i32;
                                    }
                                    bot = y as i32;
                                }
                            }
                        }
                        println!("C w {wt} dpr {scale:.2} px {px} top {top} H {h} ink {t} {bot}", t = ink_top);
                    }
                }
            }
        }
    }
}
