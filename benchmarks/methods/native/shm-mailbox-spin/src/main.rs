fn main() {
    if let Err(error) = support::run_shm_mailbox(support::WaitStrategy::Spin) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
