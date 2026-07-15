use serde_json::Value;

const MAX_COMMAND_BYTES: usize = 64 * 1024;
const MAX_TOKENS: usize = 512;
const MAX_SIMPLE_COMMANDS: usize = 64;

/// Returns whether a Bash command is conservative enough to run concurrently.
///
/// This is deliberately a small allowlist classifier, not a shell security
/// boundary. It accepts only simple commands joined by `|`, `&&`, `||`, or `;`
/// when every command is known to be read-only. Anything dynamic or ambiguous
/// (expansion, redirection, assignment, backgrounding, control flow, globbing,
/// malformed quoting, or an unknown command) fails closed.
pub fn classify_bash_concurrency(command: &str) -> bool {
    let Some(tokens) = tokenize(command) else {
        return false;
    };
    classify_tokens(&tokens)
}

/// Applies [`classify_bash_concurrency`] to the strict argument shape accepted
/// by the built-in Bash tool.
pub fn classify_bash_arguments_concurrency(arguments: &Value) -> bool {
    let Some(object) = arguments.as_object() else {
        return false;
    };
    if object
        .keys()
        .any(|key| key != "command" && key != "timeout" && key != "run_in_background")
    {
        return false;
    }
    if object
        .get("run_in_background")
        .is_some_and(|value| value.as_bool() != Some(false))
    {
        return false;
    }
    if let Some(timeout) = object.get("timeout") {
        let Some(timeout) = timeout.as_f64() else {
            return false;
        };
        if !timeout.is_finite() || timeout <= 0.0 {
            return false;
        }
    }
    object
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(classify_bash_concurrency)
}

#[derive(Debug, PartialEq, Eq)]
enum Token {
    Word(String),
    Pipe,
    AndIf,
    OrIf,
    Sequence,
}

#[derive(Clone, Copy)]
enum Quote {
    Single,
    Double,
}

fn tokenize(command: &str) -> Option<Vec<Token>> {
    if command.trim().is_empty() || command.len() > MAX_COMMAND_BYTES || command.contains('\0') {
        return None;
    }

    let mut chars = command.chars().peekable();
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut word_started = false;
    let mut quote = None;

    while let Some(character) = chars.next() {
        match quote {
            Some(Quote::Single) => {
                if character == '\'' {
                    quote = None;
                } else {
                    word.push(character);
                }
                word_started = true;
            }
            Some(Quote::Double) => match character {
                '"' => quote = None,
                '$' | '`' | '\0' => return None,
                '\\' => {
                    let escaped = chars.next()?;
                    if matches!(escaped, '\n' | '\r' | '\0') {
                        return None;
                    }
                    if !matches!(escaped, '$' | '`' | '"' | '\\') {
                        word.push('\\');
                    }
                    word.push(escaped);
                    word_started = true;
                }
                _ => {
                    word.push(character);
                    word_started = true;
                }
            },
            None => match character {
                ' ' | '\t' => finish_word(&mut tokens, &mut word, &mut word_started)?,
                '\n' | '\r' | '\0' => return None,
                '\'' => {
                    quote = Some(Quote::Single);
                    word_started = true;
                }
                '"' => {
                    quote = Some(Quote::Double);
                    word_started = true;
                }
                '\\' => {
                    let escaped = chars.next()?;
                    if matches!(escaped, '\n' | '\r' | '\0') {
                        return None;
                    }
                    word.push(escaped);
                    word_started = true;
                }
                '$' | '`' | '<' | '>' | '(' | ')' | '{' | '}' | '#' | '*' | '?' | '[' | ']' => {
                    return None;
                }
                '&' => {
                    finish_word(&mut tokens, &mut word, &mut word_started)?;
                    chars.next_if_eq(&'&')?;
                    push_token(&mut tokens, Token::AndIf)?;
                }
                '|' => {
                    finish_word(&mut tokens, &mut word, &mut word_started)?;
                    let token = if chars.next_if_eq(&'|').is_some() {
                        Token::OrIf
                    } else {
                        if chars.peek() == Some(&'&') {
                            return None;
                        }
                        Token::Pipe
                    };
                    push_token(&mut tokens, token)?;
                }
                ';' => {
                    finish_word(&mut tokens, &mut word, &mut word_started)?;
                    if chars.peek() == Some(&';') {
                        return None;
                    }
                    push_token(&mut tokens, Token::Sequence)?;
                }
                _ => {
                    word.push(character);
                    word_started = true;
                }
            },
        }
    }

    if quote.is_some() {
        return None;
    }
    finish_word(&mut tokens, &mut word, &mut word_started)?;
    (!tokens.is_empty()).then_some(tokens)
}

fn finish_word(tokens: &mut Vec<Token>, word: &mut String, word_started: &mut bool) -> Option<()> {
    if *word_started {
        push_token(tokens, Token::Word(std::mem::take(word)))?;
        *word_started = false;
    }
    Some(())
}

fn push_token(tokens: &mut Vec<Token>, token: Token) -> Option<()> {
    if tokens.len() >= MAX_TOKENS {
        return None;
    }
    tokens.push(token);
    Some(())
}

fn classify_tokens(tokens: &[Token]) -> bool {
    let mut command = Vec::new();
    let mut command_count = 0;

    for token in tokens {
        match token {
            Token::Word(word) => command.push(word.as_str()),
            Token::Pipe | Token::AndIf | Token::OrIf | Token::Sequence => {
                if !classify_simple_command(&command) {
                    return false;
                }
                command_count += 1;
                if command_count >= MAX_SIMPLE_COMMANDS {
                    return false;
                }
                command.clear();
            }
        }
    }

    !command.is_empty() && classify_simple_command(&command)
}

fn classify_simple_command(words: &[&str]) -> bool {
    let Some(command) = words.first().copied() else {
        return false;
    };
    if is_assignment(command) || command.contains('/') {
        return false;
    }

    let arguments = &words[1..];
    match command {
        "basename" | "cat" | "cut" | "df" | "dirname" | "du" | "echo" | "false" | "grep"
        | "groups" | "head" | "id" | "jq" | "locate" | "ls" | "ps" | "readlink" | "realpath"
        | "stat" | "strings" | "tail" | "test" | "tr" | "true" | "uname" | "wc" | "whoami" => true,
        "printf" => !has_option(arguments, "-v"),
        "pwd" => arguments
            .iter()
            .all(|argument| matches!(*argument, "-L" | "-P" | "--help" | "--version")),
        "rg" => !arguments.iter().any(|argument| {
            matches!(*argument, "--pre" | "--pre-glob" | "--search-zip" | "-z")
                || argument.starts_with("--pre=")
                || argument.starts_with("--pre-glob=")
        }),
        "git" => classify_git(arguments),
        _ => false,
    }
}

fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let name = name.strip_suffix('+').unwrap_or(name);
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn has_option(arguments: &[&str], option: &str) -> bool {
    arguments.contains(&option)
}

fn classify_git(arguments: &[&str]) -> bool {
    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        if !argument.starts_with('-') {
            break;
        }
        if matches!(*argument, "--no-pager" | "--no-optional-locks") {
            index += 1;
        } else {
            return false;
        }
    }

    let Some(subcommand) = arguments.get(index).copied() else {
        return false;
    };
    let subcommand_arguments = &arguments[index + 1..];

    match subcommand {
        "status" => git_status_arguments_are_read_only(subcommand_arguments),
        "branch" => git_branch_arguments_are_read_only(subcommand_arguments),
        "remote" => git_remote_arguments_are_read_only(subcommand_arguments),
        "diff" | "log" | "show" | "blame" | "grep" => subcommand_arguments
            .iter()
            .all(|argument| !is_unsafe_git_read_flag(argument)),
        "describe" | "for-each-ref" | "ls-files" | "ls-tree" | "merge-base" | "name-rev"
        | "rev-list" | "rev-parse" | "shortlog" | "show-ref" => true,
        _ => false,
    }
}

fn git_status_arguments_are_read_only(arguments: &[&str]) -> bool {
    arguments.iter().all(|argument| {
        !argument.starts_with('-')
            || matches!(
                *argument,
                "--" | "--ahead-behind"
                    | "--branch"
                    | "--ignored"
                    | "--long"
                    | "--no-ahead-behind"
                    | "--no-column"
                    | "--no-renames"
                    | "--porcelain"
                    | "--renames"
                    | "--short"
                    | "--show-stash"
                    | "--verbose"
                    | "-b"
                    | "-s"
                    | "-u"
                    | "-v"
                    | "-z"
            )
            || argument.starts_with("--column=")
            || argument.starts_with("--find-renames=")
            || argument.starts_with("--ignore-submodules=")
            || argument.starts_with("--ignored=")
            || argument.starts_with("--porcelain=")
            || argument.starts_with("--untracked-files=")
            || matches!(*argument, "-sb" | "-uno" | "-unormal" | "-uall")
    })
}

fn git_branch_arguments_are_read_only(arguments: &[&str]) -> bool {
    match arguments {
        [] | ["--show-current"] | ["--all"] | ["--remotes"] => true,
        ["--list", patterns @ ..] => patterns.iter().all(|argument| !argument.starts_with('-')),
        _ => false,
    }
}

fn git_remote_arguments_are_read_only(arguments: &[&str]) -> bool {
    match arguments {
        [] | ["-v"] => true,
        ["get-url", remote] => !remote.starts_with('-'),
        ["get-url", "--all" | "--push", remote] => !remote.starts_with('-'),
        _ => false,
    }
}

fn is_unsafe_git_read_flag(argument: &str) -> bool {
    matches!(
        argument,
        "--ext-diff" | "--textconv" | "--open-files-in-pager" | "-O"
    ) || argument == "--output"
        || argument.starts_with("--output=")
        || argument.starts_with("--open-files-in-pager=")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn accepts_known_read_only_commands() {
        for command in [
            "pwd",
            "ls -la src",
            "cat Cargo.toml",
            "rg needle src",
            "grep -n needle Cargo.toml",
            "git status --short",
            "git --no-pager log --oneline -5",
            "git diff -- src/lib.rs",
            "git branch --show-current",
            "printf '%s\\n' hello",
        ] {
            assert!(classify_bash_concurrency(command), "{command:?}");
        }
    }

    #[test]
    fn accepts_safe_pipelines_and_compound_commands() {
        for command in [
            "rg needle src | head -n 20",
            "pwd && git status --short",
            "false || ls; git diff -- src/lib.rs",
            "rg 'a|b;$(literal)' src | wc -l",
        ] {
            assert!(classify_bash_concurrency(command), "{command:?}");
        }
    }

    #[test]
    fn rejects_unknown_or_mutating_commands_and_flags() {
        for command in [
            "rm file",
            "touch file",
            "cd src",
            "xargs rm",
            "sed -i s/a/b/ file",
            "sort -o output input",
            "find . -delete",
            "git checkout main",
            "git status --unknown-option",
            "git diff --output=result.patch",
            "git grep --open-files-in-pager=vim needle",
            "rg --pre ./filter needle",
            "rg --search-zip needle archive.zip",
            "printf -v variable value",
            "/bin/ls",
        ] {
            assert!(!classify_bash_concurrency(command), "{command:?}");
        }
    }

    #[test]
    fn rejects_redirection_and_dynamic_shell_features() {
        for command in [
            "ls > files",
            "ls 2>>errors",
            "cat < input",
            "cat <<EOF",
            "cat <<< value",
            "cat <(ls)",
            "echo $(touch file)",
            "echo `touch file`",
            "echo $HOME",
            "FOO=bar git status",
            "FOO+=bar pwd",
            "ls *",
            "ls &",
            "rg needle |& head",
            "(pwd)",
            "if true; then pwd; fi",
            "pwd\nls",
        ] {
            assert!(!classify_bash_concurrency(command), "{command:?}");
        }
    }

    #[test]
    fn rejects_unsafe_pipeline_or_compound_members() {
        for command in [
            "rg needle src | tee output",
            "pwd && touch file",
            "rm file || git status",
            "git status; python build.py",
        ] {
            assert!(!classify_bash_concurrency(command), "{command:?}");
        }
    }

    #[test]
    fn rejects_malformed_or_ambiguous_syntax() {
        for command in [
            "",
            "   ",
            "'unterminated",
            "pwd &&",
            "| pwd",
            "pwd || | ls",
            "pwd ;; ls",
            "pwd # hidden comment",
            concat!("pwd ", "\\", "\n", "ls"),
        ] {
            assert!(!classify_bash_concurrency(command), "{command:?}");
        }
    }

    #[test]
    fn validates_the_bash_argument_shape() {
        assert!(classify_bash_arguments_concurrency(
            &json!({ "command": "git status", "timeout": 1.5 })
        ));
        assert!(classify_bash_arguments_concurrency(&json!({
            "command": "git status",
            "run_in_background": false
        })));
        assert!(!classify_bash_arguments_concurrency(&json!({
            "command": "git status",
            "run_in_background": true
        })));
        assert!(!classify_bash_arguments_concurrency(
            &json!({ "command": "git status", "timeout": 0 })
        ));
        assert!(!classify_bash_arguments_concurrency(
            &json!({ "command": "git status", "extra": true })
        ));
        assert!(!classify_bash_arguments_concurrency(
            &json!({ "command": "touch file" })
        ));
    }
}
