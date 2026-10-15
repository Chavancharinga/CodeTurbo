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
    CommandInfo { name: "/provedor", aliases: &["/provider"], about: "escolher o provedor de modelos", cmd: Command::Provider },
    CommandInfo {
        name: "/modelo",
        aliases: &["/model", "/models", "/modelos"],
        about: "escolher o modelo",
        cmd: Command::Model,
    },
    CommandInfo { name: "/chave", aliases: &["/key"], about: "informar a chave de API do provedor", cmd: Command::Key },
    CommandInfo {
        name: "/aprovacao",
        aliases: &["/aprovação", "/approval"],
        about: "definir quando pedir confirmação",
        cmd: Command::Approval,
    },
    CommandInfo { name: "/plano", aliases: &["/plan"], about: "ativar ou desativar o modo plano", cmd: Command::Plan },
    CommandInfo { name: "/uso", aliases: &["/usage"], about: "tokens e custo da sessão", cmd: Command::Usage },
    CommandInfo { name: "/desfazer", aliases: &["/undo"], about: "desfazer as alterações da última tarefa", cmd: Command::Undo },
    CommandInfo { name: "/regra", aliases: &["/rule"], about: "fixar uma regra que o modelo sempre deve seguir", cmd: Command::Rule },
    CommandInfo { name: "/repetir", aliases: &["/retry"], about: "refazer a última tarefa", cmd: Command::Retry },
    CommandInfo { name: "/copiar", aliases: &["/copy"], about: "copiar a última resposta", cmd: Command::Copy },
    CommandInfo { name: "/exportar", aliases: &["/export"], about: "salvar a conversa em Markdown", cmd: Command::Export },
    CommandInfo { name: "/enxugar", aliases: &["/shake"], about: "resumir resultados antigos e liberar contexto", cmd: Command::Shake },
    CommandInfo {
        name: "/limpar",
        aliases: &["/clear", "/novo", "/new"],
        about: "começar uma nova conversa",
        cmd: Command::Clear,
    },
    CommandInfo { name: "/atualizar", aliases: &["/update"], about: "atualizar o CodeTurbo", cmd: Command::Update },
    CommandInfo { name: "/ajuda", aliases: &["/help"], about: "listar os comandos e atalhos", cmd: Command::Help },
    CommandInfo { name: "/sair", aliases: &["/exit", "/quit"], about: "encerrar o CodeTurbo", cmd: Command::Exit },
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
