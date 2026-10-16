//! Slash commands typed in the input box.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Provider,
    Model,
    Key,
    Approval,
    Plan,
    Usage,
    Undo,
    Rule,
    Shake,
    Retry,
    Copy,
    Export,
    Clear,
    Update,
    Help,
    Exit,
}

pub struct CommandInfo {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub about: &'static str,
    pub cmd: Command,
}

pub const COMMANDS: [CommandInfo; 16] = [
    CommandInfo { name: "/provider", aliases: &["/provedor"], about: "choose the model provider", cmd: Command::Provider },
    CommandInfo {
        name: "/model",
        aliases: &["/models", "/modelo", "/modelos"],
        about: "choose the model",
        cmd: Command::Model,
    },
    CommandInfo { name: "/key", aliases: &["/chave"], about: "set the provider API key", cmd: Command::Key },
    CommandInfo {
        name: "/approval",
        aliases: &["/aprovacao", "/aprovação"],
        about: "choose when to ask before acting",
        cmd: Command::Approval,
    },
    CommandInfo { name: "/plan", aliases: &["/plano"], about: "turn plan mode on or off", cmd: Command::Plan },
    CommandInfo { name: "/usage", aliases: &["/uso"], about: "tokens and cost of the session", cmd: Command::Usage },
    CommandInfo { name: "/undo", aliases: &["/desfazer"], about: "undo the changes of the last task", cmd: Command::Undo },
    CommandInfo { name: "/rule", aliases: &["/regra"], about: "pin a rule the model must always follow", cmd: Command::Rule },
    CommandInfo { name: "/retry", aliases: &["/repetir"], about: "run the last task again", cmd: Command::Retry },
    CommandInfo { name: "/copy", aliases: &["/copiar"], about: "copy the last answer", cmd: Command::Copy },
    CommandInfo { name: "/export", aliases: &["/exportar"], about: "save the conversation as Markdown", cmd: Command::Export },
    CommandInfo { name: "/compact", aliases: &["/shake", "/enxugar"], about: "shrink old results and free context", cmd: Command::Shake },
    CommandInfo {
        name: "/clear",
        aliases: &["/limpar", "/novo", "/new"],
        about: "start a new conversation",
        cmd: Command::Clear,
    },
    CommandInfo { name: "/update", aliases: &["/atualizar"], about: "update CodeTurbo", cmd: Command::Update },
    CommandInfo { name: "/help", aliases: &["/ajuda"], about: "list the commands and shortcuts", cmd: Command::Help },
    CommandInfo { name: "/exit", aliases: &["/quit", "/sair"], about: "quit CodeTurbo", cmd: Command::Exit },
];

/// What a submitted line is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// A known command and the text after it.
    Command(Command, String),
    /// `/something` that is not a command.
    Unknown(String),
    /// A task for the agent.
    Task,
}

/// A line starting with `/word` (no other slash in the word, so a path
/// like `/src/main.rs` is a task) is a command.
pub fn parse(line: &str) -> Parsed {
    let line = line.trim();
    let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    if !word.starts_with('/') || word.len() < 2 || word[1..].contains('/') {
        return Parsed::Task;
    }
    let w = word.to_lowercase();
    match COMMANDS.iter().find(|c| c.name == w || c.aliases.contains(&w.as_str())) {
        Some(c) => Parsed::Command(c.cmd, rest.trim().to_string()),
        None => Parsed::Unknown(w),
    }
}

/// Commands matching a partly typed `/word`, for the suggestion list.
pub fn suggestions(typed: &str) -> Vec<&'static CommandInfo> {
    if !typed.starts_with('/') || typed.contains(char::is_whitespace) {
        return Vec::new();
    }
    let t = typed.to_lowercase();
    COMMANDS.iter().filter(|c| c.name.starts_with(&t) || c.aliases.iter().any(|a| a.starts_with(&t))).collect()
}
