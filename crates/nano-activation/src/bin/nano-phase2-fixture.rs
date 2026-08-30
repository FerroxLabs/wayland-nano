fn main() {
    match nano_activation::phase2_fixture::run(std::env::args_os().skip(1)) {
        Ok(output) => println!("{output}"),
        Err(error) => {
            eprintln!("nano-phase2-fixture: {error}");
            std::process::exit(2);
        }
    }
}
