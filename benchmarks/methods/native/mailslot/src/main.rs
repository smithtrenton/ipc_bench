fn main() {
    if let Err(error) = support::run_mailslot() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
