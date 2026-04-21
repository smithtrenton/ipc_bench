fn main() {
    if let Err(error) = support::run_udp_loopback() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
