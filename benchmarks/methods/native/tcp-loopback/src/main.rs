fn main() {
    if let Err(error) = support::run_tcp_loopback() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
