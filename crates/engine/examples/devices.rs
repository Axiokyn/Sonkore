//! Lists the outputs as the Settings page would.
fn main() {
    #[cfg(target_os = "linux")]
    println!("libpipewire {}", sonkore_engine::output::pipewire::library_version());
    for d in sonkore_engine::output::devices() {
        println!("{:<48} {:<9} {} {}", d.key, d.backend, d.name, if d.is_default { "(default)" } else { "" });
    }
}
