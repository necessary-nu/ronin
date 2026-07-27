fn main() {
    let arguments = std::env::args().collect::<Vec<_>>();
    match samurai::samu::run(&arguments, std::env::var("SAMUFLAGS").ok().as_deref()) {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
        }
        Err(error) => {
            eprintln!("samu: {error}");
            std::process::exit(1);
        }
    }
}
