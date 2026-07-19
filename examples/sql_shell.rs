use magnumdb::sql::{Executor, Parser};
use magnumdb::{Config, Database};
use std::io::{self, Write};

fn main() -> anyhow::Result<()> {
    let config = Config::default().with_path("./magnum_sql_data");
    let mut db = Database::open(config)?;
    let mut executor = Executor::new(&mut db);

    println!("MagnumDB SQL Shell");
    println!("Type 'exit' or 'quit' to exit.");

    loop {
        print!("magnum> ");
        io::stdout().flush()?;

        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            break;
        }

        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        if input.eq_ignore_ascii_case("exit") || input.eq_ignore_ascii_case("quit") {
            break;
        }

        match Parser::parse(input) {
            Ok(stmt) => match executor.execute(stmt) {
                Ok(output) => println!("{}", output),
                Err(e) => println!("Error: {}", e),
            },
            Err(e) => println!("{}", e),
        }
    }

    Ok(())
}
