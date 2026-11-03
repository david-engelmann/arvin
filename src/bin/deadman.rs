//! Command line for deadman, a thin shell over the deadman
//! library.
//!
//! Every command names the state file first. The clock the
//! command line uses is the wall clock in UTC, and no flag
//! overrides it, so the evaluation a caller sees is the
//! evaluation the machine clock gives. Errors the switch raises
//! are printed as one line on stderr with the exit code the
//! error carries, and no traceback is ever printed for them.
//!
//! The argument parsing mirrors the Python argparse surface of
//! Round 1, its usage texts and parser error messages and exit
//! codes, its acceptance of unambiguous long option prefixes,
//! its collection of unrecognized arguments at the top level,
//! and its integer conversion with the Unicode digit and
//! whitespace families of the Python int grammar, probed against
//! the Python runtime before the port was written.

use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::process::ExitCode;

use deadman::{py_repr_str, Error, Instant, Switch, Value};

const MAIN_USAGE: &str =
    "usage: deadman [-h] --state STATE {register,prove,spend,pull,stage,log} ...";

const MAIN_HELP: &str = "\
usage: deadman [-h] --state STATE {register,prove,spend,pull,stage,log} ...

Watch agent runs and walk them down a staged cutoff as the evidence of a
runaway grows.

positional arguments:
  {register,prove,spend,pull,stage,log}
    register            open a run record
    prove               record a progress proof for a run
    spend               record a spend report for a run
    pull                pull the switch and kill the run
    stage               evaluate a run and report its stage
    log                 show the recorded decision log of a run

options:
  -h, --help            show this help message and exit
  --state STATE         the state file holding the runs, created by the first
                        mutation";

const COMMANDS: [&str; 6] = ["register", "prove", "spend", "pull", "stage", "log"];

fn sub_usage(command: &str) -> &'static str {
    match command {
        "register" => {
            "usage: deadman register [-h] [--proof-interval PROOF_INTERVAL]\n                        [--missed-limit MISSED_LIMIT]\n                        [--soft-ceiling SOFT_CEILING]\n                        [--hard-ceiling HARD_CEILING]\n                        run"
        }
        "prove" => "usage: deadman prove [-h] [--note NOTE] run",
        "spend" => "usage: deadman spend [-h] --amount AMOUNT run",
        "pull" => "usage: deadman pull [-h] run",
        "stage" => "usage: deadman stage [-h] run",
        _ => "usage: deadman log [-h] run",
    }
}

fn sub_help(command: &str) -> &'static str {
    match command {
        "register" => {
            "usage: deadman register [-h] [--proof-interval PROOF_INTERVAL]\n                        [--missed-limit MISSED_LIMIT]\n                        [--soft-ceiling SOFT_CEILING]\n                        [--hard-ceiling HARD_CEILING]\n                        run\n\npositional arguments:\n  run\n\noptions:\n  -h, --help            show this help message and exit\n  --proof-interval PROOF_INTERVAL\n                        the proof interval in seconds, default 60\n  --missed-limit MISSED_LIMIT\n                        the missed proof limit that warrants kill, default 5\n  --soft-ceiling SOFT_CEILING\n                        the spend at or above which cut budget is warranted\n  --hard-ceiling HARD_CEILING\n                        the spend at or above which kill is warranted"
        }
        "prove" => {
            "usage: deadman prove [-h] [--note NOTE] run\n\npositional arguments:\n  run\n\noptions:\n  -h, --help   show this help message and exit\n  --note NOTE  a short progress note, never judged"
        }
        "spend" => {
            "usage: deadman spend [-h] --amount AMOUNT run\n\npositional arguments:\n  run\n\noptions:\n  -h, --help       show this help message and exit\n  --amount AMOUNT  the spend to add, in host units"
        }
        "pull" => {
            "usage: deadman pull [-h] run\n\npositional arguments:\n  run\n\noptions:\n  -h, --help  show this help message and exit"
        }
        "stage" => {
            "usage: deadman stage [-h] run\n\npositional arguments:\n  run\n\noptions:\n  -h, --help  show this help message and exit"
        }
        _ => {
            "usage: deadman log [-h] run\n\npositional arguments:\n  run\n\noptions:\n  -h, --help  show this help message and exit"
        }
    }
}

fn value_options(command: &str) -> &'static [&'static str] {
    match command {
        "register" => &[
            "--proof-interval",
            "--missed-limit",
            "--soft-ceiling",
            "--hard-ceiling",
        ],
        "prove" => &["--note"],
        "spend" => &["--amount"],
        _ => &[],
    }
}

fn is_int_option(command: &str, option: &str) -> bool {
    match command {
        "register" => matches!(
            option,
            "--proof-interval" | "--missed-limit" | "--soft-ceiling" | "--hard-ceiling"
        ),
        "spend" => option == "--amount",
        _ => false,
    }
}

fn required_options(command: &str) -> &'static [&'static str] {
    match command {
        "spend" => &["--amount"],
        _ => &[],
    }
}

struct ParserError {
    usage: &'static str,
    prog: String,
    message: String,
}

fn parser_error(usage: &'static str, prog: &str, message: String) -> ParserError {
    ParserError {
        usage,
        prog: prog.to_string(),
        message,
    }
}

fn parser_fail(usage: &str, prog: &str, message: &str) -> ExitCode {
    eprintln!("{usage}");
    eprintln!("{prog}: error: {message}");
    ExitCode::from(2)
}

fn is_option_like(token: &str) -> bool {
    if token.len() < 2 || !token.starts_with('-') || token == "-" {
        return false;
    }
    // A token in the shape of a negative number is positional,
    // as argparse's negative number matcher holds when no
    // option looks like a negative number. The digits of that
    // shape are the decimal digits of the Python grammar, the
    // same set the integer conversion below accepts.
    let body = &token[1..];
    let digits: String = body.chars().filter(|c| *c != '.').collect();
    if !digits.is_empty() && digits.chars().all(|c| py_digit_value(c).is_some()) {
        return false;
    }
    true
}

/// Resolve a long option token against the candidates of one
/// parser, the argparse way: an exact match wins, an
/// unambiguous prefix matches, an ambiguous prefix is an
/// error, and no match means the token is an extra.
fn resolve_long<'a>(name: &'a str, candidates: &[&'a str]) -> Result<Option<&'a str>, String> {
    if candidates.contains(&name) {
        return Ok(Some(name));
    }
    let matches: Vec<&str> = candidates
        .iter()
        .filter(|candidate| candidate.starts_with(name))
        .copied()
        .collect();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches[0])),
        _ => Err(format!(
            "ambiguous option: {name} could match {}",
            matches.join(", ")
        )),
    }
}

enum SubValue {
    Text(String),
    Int(Value),
}

struct SubArgs {
    positionals: Vec<String>,
    options: Vec<(String, SubValue)>,
    extras: Vec<String>,
}

/// The decimal digit ranges of the Unicode character set,
/// each ten codepoints wide with the digit value standing at
/// the offset inside its range, as probed against the Python
/// runtime the conversion mirrors, Python 3.12.3 carrying
/// Unicode 15.0.0. The Python int conversion accepts a digit
/// from any of these families and no other numeric character.
const DECIMAL_DIGIT_RANGES: [(u32, u32); 68] = [
    (0x0030, 0x0039),
    (0x0660, 0x0669),
    (0x06f0, 0x06f9),
    (0x07c0, 0x07c9),
    (0x0966, 0x096f),
    (0x09e6, 0x09ef),
    (0x0a66, 0x0a6f),
    (0x0ae6, 0x0aef),
    (0x0b66, 0x0b6f),
    (0x0be6, 0x0bef),
    (0x0c66, 0x0c6f),
    (0x0ce6, 0x0cef),
    (0x0d66, 0x0d6f),
    (0x0de6, 0x0def),
    (0x0e50, 0x0e59),
    (0x0ed0, 0x0ed9),
    (0x0f20, 0x0f29),
    (0x1040, 0x1049),
    (0x1090, 0x1099),
    (0x17e0, 0x17e9),
    (0x1810, 0x1819),
    (0x1946, 0x194f),
    (0x19d0, 0x19d9),
    (0x1a80, 0x1a89),
    (0x1a90, 0x1a99),
    (0x1b50, 0x1b59),
    (0x1bb0, 0x1bb9),
    (0x1c40, 0x1c49),
    (0x1c50, 0x1c59),
    (0xa620, 0xa629),
    (0xa8d0, 0xa8d9),
    (0xa900, 0xa909),
    (0xa9d0, 0xa9d9),
    (0xa9f0, 0xa9f9),
    (0xaa50, 0xaa59),
    (0xabf0, 0xabf9),
    (0xff10, 0xff19),
    (0x104a0, 0x104a9),
    (0x10d30, 0x10d39),
    (0x11066, 0x1106f),
    (0x110f0, 0x110f9),
    (0x11136, 0x1113f),
    (0x111d0, 0x111d9),
    (0x112f0, 0x112f9),
    (0x11450, 0x11459),
    (0x114d0, 0x114d9),
    (0x11650, 0x11659),
    (0x116c0, 0x116c9),
    (0x11730, 0x11739),
    (0x118e0, 0x118e9),
    (0x11950, 0x11959),
    (0x11c50, 0x11c59),
    (0x11d50, 0x11d59),
    (0x11da0, 0x11da9),
    (0x11f50, 0x11f59),
    (0x16a60, 0x16a69),
    (0x16ac0, 0x16ac9),
    (0x16b50, 0x16b59),
    (0x1d7ce, 0x1d7d7),
    (0x1d7d8, 0x1d7e1),
    (0x1d7e2, 0x1d7eb),
    (0x1d7ec, 0x1d7f5),
    (0x1d7f6, 0x1d7ff),
    (0x1e140, 0x1e149),
    (0x1e2f0, 0x1e2f9),
    (0x1e4f0, 0x1e4f9),
    (0x1e950, 0x1e959),
    (0x1fbf0, 0x1fbf9),
];

/// The value of one character under the Python int conversion,
/// a decimal digit in any family of the table above, or None
/// for any other character.
fn py_digit_value(c: char) -> Option<u32> {
    let n = c as u32;
    DECIMAL_DIGIT_RANGES
        .iter()
        .find(|&&(low, high)| n >= low && n <= high)
        .map(|&(low, _)| n - low)
}

/// The whitespace the Python int conversion strips, the
/// Py_UNICODE_ISSPACE set, which is wider than the Rust
/// whitespace property by the information separators.
fn is_python_space(c: char) -> bool {
    matches!(c,
        '\u{0009}'..='\u{000d}'
        | '\u{001c}'..='\u{001f}'
        | ' '
        | '\u{0085}'
        | '\u{00a0}'
        | '\u{1680}'
        | '\u{2000}'..='\u{200a}'
        | '\u{2028}'
        | '\u{2029}'
        | '\u{202f}'
        | '\u{205f}'
        | '\u{3000}')
}

/// Python's int conversion for the numeric arguments: optional
/// surrounding whitespace from the Python whitespace set, an
/// optional sign, decimal digits from any family of the table
/// above with single underscores between them. The conversion
/// refuses a digit count past 4300 digit characters, the limit
/// the Python runtime applies, where a wider conversion would
/// stand as a parser failure where Python converted the value.
/// The value is kept whole, at 128 bit width when it fits and
/// as its canonical digit text when it does not, so a later
/// validation prints the number the caller typed.
fn python_int(text: &str) -> Option<Value> {
    let trimmed = text.trim_matches(is_python_space);
    let (negative, body) = if let Some(rest) = trimmed.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = trimmed.strip_prefix('+') {
        (false, rest)
    } else {
        (false, trimmed)
    };
    if body.is_empty() {
        return None;
    }
    let mut digits = String::new();
    let mut digit_count = 0usize;
    let mut previous_digit = false;
    let mut previous_underscore = false;
    let mut seen_digit = false;
    for ch in body.chars() {
        if ch == '_' {
            if !previous_digit || previous_underscore {
                return None;
            }
            previous_underscore = true;
            previous_digit = false;
        } else if let Some(value) = py_digit_value(ch) {
            digits.push(char::from_digit(value, 10).unwrap());
            digit_count += 1;
            previous_digit = true;
            previous_underscore = false;
            seen_digit = true;
        } else {
            return None;
        }
    }
    if previous_underscore || !seen_digit {
        return None;
    }
    if digit_count > 4300 {
        return None;
    }
    let canonical_digits = {
        let stripped = digits.trim_start_matches('0');
        if stripped.is_empty() {
            "0".to_string()
        } else {
            stripped.to_string()
        }
    };
    let signed = if negative && canonical_digits != "0" {
        format!("-{canonical_digits}")
    } else {
        canonical_digits.clone()
    };
    match signed.parse::<i128>() {
        Ok(number) => Some(Value::Int(number)),
        Err(_) => Some(Value::BigInt(signed)),
    }
}

/// Parse one subcommand's arguments. Option values are drawn
/// from the following token unless it is option like, numeric
/// options are converted as they are consumed, and tokens the
/// subcommand does not consume are collected as extras for the
/// top level parser to report. A double dash stops option
/// handling and is itself consumed, as the Python parser
/// consumed it below the subcommand.
fn parse_sub(command: &str, tokens: &[String]) -> Result<SubArgs, ParserError> {
    let mut candidates: Vec<&str> = vec!["--help"];
    candidates.extend_from_slice(value_options(command));
    let mut result = SubArgs {
        positionals: Vec::new(),
        options: Vec::new(),
        extras: Vec::new(),
    };
    let mut index = 0;
    let mut after_separator = false;
    while index < tokens.len() {
        let token = &tokens[index];
        if !after_separator && token == "--" {
            after_separator = true;
            index += 1;
            continue;
        }
        if !after_separator
            && (token == "-h"
                || token == "--help"
                || (token.starts_with("-h") && !token.starts_with("--")))
        {
            println!("{}", sub_help(command));
            std::process::exit(0);
        }
        if !after_separator && token.starts_with("--") && token.len() > 2 {
            let (name, inline_value) = match token.find('=') {
                Some(pos) => (&token[..pos], Some(token[pos + 1..].to_string())),
                None => (&token[..], None),
            };
            match resolve_long(name, &candidates) {
                Ok(Some("--help")) => {
                    if let Some(value) = inline_value {
                        return Err(parser_error(
                            sub_usage(command),
                            &format!("deadman {command}"),
                            format!(
                                "argument -h/--help: ignored explicit argument {}",
                                py_repr_str(&value)
                            ),
                        ));
                    }
                    println!("{}", sub_help(command));
                    std::process::exit(0);
                }
                Ok(Some(option)) => {
                    let raw = if let Some(value) = inline_value {
                        index += 1;
                        value
                    } else if index + 1 < tokens.len() && !is_option_like(&tokens[index + 1]) {
                        let value = tokens[index + 1].clone();
                        index += 2;
                        value
                    } else {
                        return Err(parser_error(
                            sub_usage(command),
                            &format!("deadman {command}"),
                            format!("argument {option}: expected one argument"),
                        ));
                    };
                    if is_int_option(command, option) {
                        match python_int(&raw) {
                            Some(value) => {
                                result
                                    .options
                                    .push((option.to_string(), SubValue::Int(value)));
                            }
                            None => {
                                return Err(parser_error(
                                    sub_usage(command),
                                    &format!("deadman {command}"),
                                    format!(
                                        "argument {option}: invalid int value: {}",
                                        py_repr_str(&raw)
                                    ),
                                ));
                            }
                        }
                    } else {
                        result
                            .options
                            .push((option.to_string(), SubValue::Text(raw)));
                    }
                    continue;
                }
                Ok(None) => {
                    result.extras.push(token.clone());
                    index += 1;
                    continue;
                }
                Err(message) => {
                    return Err(parser_error(
                        sub_usage(command),
                        &format!("deadman {command}"),
                        message,
                    ));
                }
            }
        }
        if !after_separator && is_option_like(token) {
            result.extras.push(token.clone());
            index += 1;
            continue;
        }
        if result.positionals.is_empty() {
            result.positionals.push(token.clone());
        } else {
            result.extras.push(token.clone());
        }
        index += 1;
    }
    Ok(result)
}

fn option_text<'a>(args: &'a SubArgs, name: &str) -> Option<&'a str> {
    args.options.iter().rev().find_map(|(seen, value)| {
        if seen == name {
            match value {
                SubValue::Text(text) => Some(text.as_str()),
                SubValue::Int(_) => None,
            }
        } else {
            None
        }
    })
}

fn option_int<'a>(args: &'a SubArgs, name: &str) -> Option<&'a Value> {
    args.options.iter().rev().find_map(|(seen, value)| {
        if seen == name {
            match value {
                SubValue::Int(value) => Some(value),
                SubValue::Text(_) => None,
            }
        } else {
            None
        }
    })
}

fn require(command: &str, args: &SubArgs) -> Result<(), ParserError> {
    let mut missing: Vec<String> = Vec::new();
    if args.positionals.is_empty() {
        missing.push("run".to_string());
    }
    for name in required_options(command) {
        if option_int(args, name).is_none() {
            missing.push(name.to_string());
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(parser_error(
            sub_usage(command),
            &format!("deadman {command}"),
            format!(
                "the following arguments are required: {}",
                missing.join(", ")
            ),
        ))
    }
}

fn report_error(error: &Error) -> ExitCode {
    eprintln!("error: {}", error.message());
    ExitCode::from(error.exit_code())
}

fn run() -> ExitCode {
    let os_argv: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let argv: Vec<String> = os_argv
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    match dispatch(&argv, &os_argv) {
        Ok(code) => code,
        Err(error) => report_error(&error),
    }
}

fn dispatch(argv: &[String], os_argv: &[std::ffi::OsString]) -> Result<ExitCode, Error> {
    // Parse the top level: the state option and the subcommand
    // word, with the remaining tokens handed to the subcommand.
    // A double dash standing where the subcommand word is
    // expected is taken as the subcommand word itself, and a
    // double dash with nothing after it is taken as nothing,
    // both as the Python parser took them.
    let mut state_path: Option<PathBuf> = None;
    let mut index = 0;
    let mut command: Option<String> = None;
    let mut rest: Vec<String> = Vec::new();
    let mut extras: Vec<String> = Vec::new();
    while index < argv.len() {
        let token = &argv[index];
        if token == "--" {
            if index + 1 < argv.len() {
                command = Some(token.clone());
                rest = argv[index + 1..].to_vec();
            }
            break;
        }
        if token == "-h"
            || token == "--help"
            || (token.starts_with("-h") && !token.starts_with("--"))
        {
            println!("{MAIN_HELP}");
            return Ok(ExitCode::from(0));
        }
        if token.starts_with("--") && token.len() > 2 {
            let (name, inline_value) = match token.find('=') {
                Some(pos) => (&token[..pos], Some(token[pos + 1..].to_string())),
                None => (&token[..], None),
            };
            match resolve_long(name, &["--help", "--state"]) {
                Ok(Some("--help")) => {
                    if let Some(value) = inline_value {
                        return Ok(parser_fail(
                            MAIN_USAGE,
                            "deadman",
                            &format!(
                                "argument -h/--help: ignored explicit argument {}",
                                py_repr_str(&value)
                            ),
                        ));
                    }
                    println!("{MAIN_HELP}");
                    return Ok(ExitCode::from(0));
                }
                Ok(Some("--state")) => {
                    if inline_value.is_some() {
                        // The state value is carried as the raw
                        // argument bytes, so a state path that is
                        // not valid UTF-8 names the file the caller
                        // named, as the Python surface names it.
                        let raw = os_argv[index].as_bytes();
                        let at = raw.iter().position(|byte| *byte == b'=').unwrap();
                        state_path = Some(PathBuf::from(std::ffi::OsString::from_vec(
                            raw[at + 1..].to_vec(),
                        )));
                        index += 1;
                    } else if index + 1 < argv.len() && !is_option_like(&argv[index + 1]) {
                        state_path = Some(PathBuf::from(os_argv[index + 1].clone()));
                        index += 2;
                    } else {
                        return Ok(parser_fail(
                            MAIN_USAGE,
                            "deadman",
                            "argument --state: expected one argument",
                        ));
                    }
                    continue;
                }
                Ok(Some(_)) => unreachable!(),
                Ok(None) => {
                    extras.push(token.clone());
                    index += 1;
                    continue;
                }
                Err(message) => {
                    return Ok(parser_fail(MAIN_USAGE, "deadman", &message));
                }
            }
        }
        if is_option_like(token) {
            extras.push(token.clone());
            index += 1;
            continue;
        }
        command = Some(token.clone());
        rest = argv[index + 1..].to_vec();
        break;
    }
    let Some(command) = command else {
        let mut missing = Vec::new();
        if state_path.is_none() {
            missing.push("--state");
        }
        missing.push("command");
        return Ok(parser_fail(
            MAIN_USAGE,
            "deadman",
            &format!(
                "the following arguments are required: {}",
                missing.join(", ")
            ),
        ));
    };
    if !COMMANDS.contains(&command.as_str()) {
        return Ok(parser_fail(
            MAIN_USAGE,
            "deadman",
            &format!(
                "argument command: invalid choice: {} (choose from 'register', 'prove', 'spend', 'pull', 'stage', 'log')",
                py_repr_str(&command)
            ),
        ));
    }
    let args = match parse_sub(&command, &rest) {
        Ok(args) => args,
        Err(failure) => {
            return Ok(parser_fail(failure.usage, &failure.prog, &failure.message));
        }
    };
    if let Err(failure) = require(&command, &args) {
        return Ok(parser_fail(failure.usage, &failure.prog, &failure.message));
    }
    if state_path.is_none() {
        return Ok(parser_fail(
            MAIN_USAGE,
            "deadman",
            "the following arguments are required: --state",
        ));
    }
    extras.extend(args.extras.iter().cloned());
    if !extras.is_empty() {
        return Ok(parser_fail(
            MAIN_USAGE,
            "deadman",
            &format!("unrecognized arguments: {}", extras.join(" ")),
        ));
    }
    let switch = Switch::new(state_path.unwrap());
    let now = Instant::now_utc();
    let run = args.positionals.first().cloned().unwrap_or_default();
    let run_value = Value::Str(run.clone());
    match command.as_str() {
        "register" => {
            let proof_interval = option_int(&args, "--proof-interval")
                .cloned()
                .unwrap_or(Value::Int(60));
            let missed_limit = option_int(&args, "--missed-limit")
                .cloned()
                .unwrap_or(Value::Int(5));
            let soft_ceiling = option_int(&args, "--soft-ceiling")
                .cloned()
                .unwrap_or(Value::Null);
            let hard_ceiling = option_int(&args, "--hard-ceiling")
                .cloned()
                .unwrap_or(Value::Null);
            switch.register(
                &run_value,
                &now,
                &proof_interval,
                &missed_limit,
                &soft_ceiling,
                &hard_ceiling,
            )?;
            println!("registered {run}");
        }
        "prove" => {
            let note = option_text(&args, "--note").unwrap_or("").to_string();
            switch.prove(&run_value, &Value::Str(note), &now)?;
            println!("proved {run}");
        }
        "spend" => {
            let amount = option_int(&args, "--amount").unwrap().clone();
            let total = switch.spend(&run_value, &amount, &now)?;
            println!("spend_total {total}");
        }
        "pull" => {
            let stage = switch.pull(&run_value, &now)?;
            println!("stage {stage}");
        }
        "stage" => {
            let report = switch.stage(&run_value, &now)?;
            println!("stage {}", report.stage);
            for reason in &report.reasons {
                println!("reason {reason}");
            }
        }
        _ => {
            for record in switch.log(&run_value)? {
                let mut line = format!(
                    "{} {} at={} stage={}",
                    record.seq, record.kind, record.at, record.stage
                );
                for reason in &record.reasons {
                    line.push_str(&format!(" reason={reason}"));
                }
                println!("{line}");
            }
        }
    }
    Ok(ExitCode::from(0))
}

fn main() -> ExitCode {
    run()
}
