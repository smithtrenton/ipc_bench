fn main() {
    if let Err(error) = support::run_shm_semaphores() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
