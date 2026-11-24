fn main() {
    let path = std::env::args().nth(1).unwrap();
    println!("{:?}", sonkore_engine::decode::mp4_edit(std::path::Path::new(&path), 44100));
}
