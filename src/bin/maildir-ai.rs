use arrrg::CommandLine;
use utf8path::Path;

use maildir_ai::{init, maintain, MaintainOptions};

#[derive(Clone, Debug, Default, Eq, PartialEq, arrrg_derive::CommandLine)]
struct Options {}

fn help() {
    eprintln!(
        "maildir-ai - maildir-driven artificial intelligence

commands:
init        initialize a new maildir-ai database
run         invoke mutt configured to access the current maildir-ai database
maintain    maintain the maildir-ai database
"
    );
}

#[tokio::main]
async fn main() {
    let (options, args) =
        Options::from_command_line_relaxed("USAGE: maildir-ai [OPTIONS] <command>");
    if args.is_empty() {
        help();
        std::process::exit(1);
    }
    match args[0].as_str() {
        "init" => {
            if args.len() != 3 {
                eprintln!("expected exactly two arguments for the init command");
                eprintln!("USAGE: maildir-ai init <knowledge-base> <name>");
                std::process::exit(1);
            }
            let knowledge_base = Path::new(&args[1]);
            init(&knowledge_base, &args[2]).expect("failed to initialize knowledge base");
        }
        "mail" => {
            if args.len() != 2 {
                eprintln!("expected exactly one argument for the run command");
                eprintln!("USAGE: maildir-ai mail <knowledge-base>");
                std::process::exit(1);
            }
            let knowledge_base = Path::new(&args[1]);
            std::process::Command::new("mutt")
                .arg("-F")
                .arg(knowledge_base.join(".muttrc"))
                .status()
                .expect("mutt failed to start");
        }
        "run" => {
            if args.len() != 2 {
                eprintln!("expected exactly one argument for the run command");
                eprintln!("USAGE: maildir-ai run <knowledge-base>");
                std::process::exit(1);
            }
            let knowledge_base = Path::new(&args[1]);
            if !knowledge_base.join(".muttrc").clone().into_std().is_file() {
                eprintln!("knowledge base is missing .muttrc: {}", knowledge_base);
                std::process::exit(1);
            }
            std::process::Command::new("mutt")
                .arg("-F")
                .arg(knowledge_base.join(".muttrc"))
                .status()
                .expect("mutt failed to start");
        }
        "maintain" => {
            let args = args.iter().map(|s| s.as_str()).collect::<Vec<&str>>();
            let _ = options;
            let (options, args) = MaintainOptions::from_arguments(
                "USAGE: maildir-ai maintain [OPTIONS] <knowledge-base>",
                &args[1..],
            );
            if args.len() != 1 {
                eprintln!("expected exactly one argument for the run command");
                eprintln!("USAGE: maildir-ai run <knowledge-base>");
                std::process::exit(1);
            }
            let knowledge_base = Path::new(&args[0]);
            maintain(&options, &knowledge_base).await;
        }
        "format-reply" => {
            for arg in args.iter().skip(1) {
                let path = Path::new(arg);
                if !path.clone().into_std().is_file() {
                    eprintln!("not a file: {}", path);
                    continue;
                }
                let content = std::fs::read_to_string(&path).unwrap();
                let formatted = maildir_ai::format_reply("cl4p-tp@rave", &content).unwrap();
                println!("{}\n", formatted);
            }
        }
        _ => {
            eprintln!("unknown command: {}\n", args[0]);
            help();
            std::process::exit(1);
        }
    }
}
