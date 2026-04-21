fn main() {
    if let Err(error) = support::run_af_unix() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
