fn main() {
    if let Err(error) = support::run_shm_events() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
