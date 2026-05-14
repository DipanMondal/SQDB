mod error;
mod model;
mod language;
mod engine;
mod storage;

use std::io::{self, Write};

use crate::engine::Engine;
use crate::language::parser::parse_command;

fn main() {
    println!("SQDB - Stack Queue Database");
    println!("Type `help;` to see commands.");
    println!("Type `exit;` to quit.");
    println!();

    let mut engine = Engine::new();

    loop {
        print!("sqdb> ");
        io::stdout().flush().expect("Failed to flush stdout");

        let mut input = String::new();

        match io::stdin().read_line(&mut input) {
            Ok(0) => {
                println!("Goodbye.");
                break;
            }
            Ok(_) => {}
            Err(err) => {
                eprintln!("Input error: {}", err);
                continue;
            }
        }

        let input = input.trim();

        if input.is_empty() {
            continue;
        }

        match parse_command(input) {
            Ok(command) => {
                if command.is_exit() {
                    println!("Goodbye.");
                    break;
                }

                match engine.execute(command) {
                    Ok(output) => println!("{}", output),
                    Err(err) => eprintln!("Error: {}", err),
                }
            }
            Err(err) => {
                eprintln!("Error: {}", err);
            }
        }
    }
}