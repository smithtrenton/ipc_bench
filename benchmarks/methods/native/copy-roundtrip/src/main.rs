fn main() {
    if let Err(error) = support::run_copy_roundtrip() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
