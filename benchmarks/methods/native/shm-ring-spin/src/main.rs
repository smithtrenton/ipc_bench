fn main() {
    if let Err(error) = support::run_shm_ring(support::WaitStrategy::Spin) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
