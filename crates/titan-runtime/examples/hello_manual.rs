//! Critério de aceite da T0: um `main` escrito à mão que usa o runtime.
//!
//! É a prova de que a fundação funciona *antes* de existir compilador — este
//! arquivo é a forma que o `titanc` vai gerar automaticamente a partir de
//! `examples/hello.titan` na T6/T7.
//!
//! ```bash
//! cargo run -p titan-runtime --example hello_manual
//! ```

fn titan_main(_args: &[String]) -> i64 {
    titan_runtime::print("Olá, mundo!");
    0
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(titan_main(&args) as i32);
}
