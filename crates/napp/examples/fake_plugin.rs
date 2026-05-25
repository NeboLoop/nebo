use std::collections::HashMap;
use std::io::Read;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let subcmd = args.get(1).map(|s| s.as_str()).unwrap_or("help");

    match subcmd {
        "echo-env" => {
            let env: HashMap<String, String> = std::env::vars().collect();
            println!("{}", serde_json::to_string(&env).unwrap());
        }
        "echo-args" => {
            let rest: Vec<&str> = args[2..].iter().map(|s| s.as_str()).collect();
            println!("{}", serde_json::to_string(&rest).unwrap());
        }
        "echo-stdin" => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf).unwrap();
            print!("{}", buf);
        }
        "sleep" => {
            let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);
            std::thread::sleep(std::time::Duration::from_secs(secs));
            println!("slept {}s", secs);
        }
        "ndjson" => {
            println!(r#"{{"type":"text","content":"hello"}}"#);
            println!(r#"{{"type":"done","usage":{{"input_tokens":10,"output_tokens":20}}}}"#);
        }
        _ => {
            eprintln!("unknown subcommand: {}", subcmd);
            std::process::exit(1);
        }
    }
}
