//! Reads a settings INI file and writes it back: the bytes must not change.
fn main() {
    let path = std::env::args().nth(1).expect("a settings file");
    let original = std::fs::read(&path).expect("readable");
    let again = sonkore_core::settings::write(&sonkore_core::settings::parse(&original));
    if again == original {
        println!("identical ({} bytes)", original.len());
    } else {
        let a = String::from_utf8_lossy(&original);
        let b = String::from_utf8_lossy(&again);
        for (i, (x, y)) in a.lines().zip(b.lines()).enumerate() {
            if x != y {
                println!("line {}:\n  before: {}\n  now:    {}", i + 1, &x[..x.len().min(160)], &y[..y.len().min(160)]);
            }
        }
        println!("lines: before {} now {}", a.lines().count(), b.lines().count());
        std::process::exit(1);
    }
}
