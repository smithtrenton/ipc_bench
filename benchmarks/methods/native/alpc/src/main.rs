fn main() {
    if let Err(error) = support::run_alpc() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
