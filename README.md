<p align="center">
  <img src="docs/codeturbo.svg" alt="CodeTurbo: o logo com um caracol correndo" width="618">
</p>

<p align="center">
  <a href="#licença"><img alt="Licença: MIT OR Apache-2.0" src="https://img.shields.io/badge/licen%C3%A7a-MIT%20OR%20Apache--2.0-2F7BFF"></a>
  <img alt="Rust 1.94+" src="https://img.shields.io/badge/rust-1.94%2B-2F7BFF?logo=rust&logoColor=white">
  <img alt="100% Rust" src="https://img.shields.io/badge/100%25-Rust-2F7BFF">
  <img alt="Status: em desenvolvimento" src="https://img.shields.io/badge/status-em%20desenvolvimento-555555">
</p>

<p align="center"><b>Agente de código em Rust</b> · fast · frugal · verified</p>

---

CodeTurbo é um agente de código de terminal escrito 100% em Rust. Ele lê e edita
o seu projeto com ferramentas que gastam poucos tokens e só dá uma tarefa como
concluída depois de compilar, passar no lint e nos testes de verdade.

## O que ele faz

- **Economia de tokens**: lê arquivos Rust grandes como esqueleto e edita por
  símbolo (`read_symbol`, `replace_symbol`), com cache de prompt e histórico
  append-only.
- **Verificação real**: ao terminar, roda build, `clippy -D warnings` e testes.
  Zero erros e zero alertas, senão o modelo recebe os diagnósticos e corrige.
  Um anti-trapaça recusa testes apagados, `#[ignore]` e lints silenciados.
- **Diagnóstico a cada edição**, embutido e em Rust: Rust, JS/TS, CSS, HTML,
  JSON e TOML, sem servidor externo. Um servidor LSP, se instalado, acrescenta
  checagem de tipos.
- **Confirmações**: modos `always-ask`, `write` e `yolo`, política por
  ferramenta e por comando, e diff colorido antes de cada edição.
- **Modo plano**: somente leitura até você aprovar o plano.
- **Qualquer provedor**: Anthropic, OpenAI, OpenRouter e modelos locais via
  Ollama.
- **No terminal**: thinking do modelo, diffs com número de linha, tokens,
  tok/s, contexto, custo e limites a cada resposta.

## Começar

```bash
cargo build --release
```

Com um modelo local (Ollama, sem chave):

```bash
cargo run -p coturbo-cli -- --provider ollama --model qwen2.5:7b
```

Com o Claude (defina `ANTHROPIC_API_KEY`):

```bash
cargo run -p coturbo-cli -- --model claude-sonnet-5-5
```

Uma tarefa só, sem REPL:

```bash
cargo run -p coturbo-cli -- run "adicione um método que soma dois pontos"
```

Configuração em `%APPDATA%\coturbo\config.toml` (Windows) ou
`~/.config/coturbo/config.toml`.

## Status

Em desenvolvimento. Próximos passos: TUI, mais provedores, benchmark de
economia de tokens e o motor de design embutido.

## Licença

Licenciado sob a sua escolha de uma das duas:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

Qualquer contribuição enviada para inclusão no projeto, salvo indicação em
contrário, fica licenciada da mesma forma, sem termos adicionais.
