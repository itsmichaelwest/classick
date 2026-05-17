mod ffi;

fn main() {
    // Sanity check that bindings compiled.
    println!("ipod-sync build.rs + bindgen wired up");
    println!("size of Itdb_Track: {}", std::mem::size_of::<ffi::Itdb_Track>());
}
