# CoTurbo MVP — agente de código em Rust puro, focado em economia de tokens

## Context
Agentes de código (OMP, Claude Code, Aider) queimam tokens lendo arquivos inteiros, reescrevendo blocos grandes e reenviando logs de erro. A tese: ferramentas baseadas em AST + contexto compactado + prompt caching gastam muito menos com a mesma taxa de sucesso. Objetivo atual do usuário: **(A) validar a tese com números reproduzíveis** e **(C) lançar open-source** depois. Decisões tomadas no brainstorming:
- CLI agente mínimo completo (não fork, não só MCP), **100% Rust** (sem Node/TS; parser sem C).
- Provedores: Anthropic, OpenRouter, OpenAI, Ollama (BYOK por env var; sem OAuth).
- Linguagem indexada: só Rust, via `ra_ap_syntax` (lossless, tolerante a erro).
- Interface: REPL simples (`rustyline`) + linha de status de tokens/custo.
- Validação: benchmark A/B interno `--tools naive` vs `--tools ast`.
- REA (morluto/rea) = ferramenta de engenharia reversa; fica **fora do produto**, uso opcional de pesquisa só com aprovação explícita (não usar em Claude Code — licença proíbe RE).

Ambiente: pasta `C:\Users\danie\Music\CoTurbon` vazia, sem git. cargo/rustc 1.94, rustfmt 1.8, git 2.52 disponíveis.

## Passo 0 — Setup e spec
1. `git init` em `CoTurbon`, `.gitignore` (`target/`, `bench/runs/`).
2. Copiar este documento como spec em `docs/superpowers/specs/2026-10-13-coturbo-mvp-design.md`; commit.

## Estrutura (workspace Cargo)
```
Cargo.toml                (workspace, edition 2024)
crates/coturbo-providers  Provider trait + anthropic + openai_compat + usage/pricing
crates/coturbo-ast        ra_ap_syntax: skeleton, read_symbol, replace/insert_symbol
crates/coturbo-tools      ToolSet naive | ast; cargo_check compacto; bash; grep
crates/coturbo-agent      loop do agente + ContextManager (cache-stable prefix, compactação)
crates/coturbo-cli        bin `coturbo`: repl, run, bench
bench/fixture/            crate Rust pequeno com testes
bench/tasks/*.toml        { id, prompt, verify = "cargo test ..." }
pricing.toml              preço por modelo (input, cache_read, cache_write, output)
```
Dependências principais (todas Rust): `tokio`, `reqwest` (feature `rustls-tls`, sem openssl), `eventsource-stream`/parser SSE próprio, `serde`/`serde_json`, `toml`, `clap`, `rustyline`, `ra_ap_syntax`, `ignore`, `regex`, `thiserror`/`anyhow`, `tempfile`. `rustfmt` chamado como subprocesso.

## Fases de implementação (TDD em cada uma)
1. **coturbo-ast** (núcleo da tese, sem rede)
   - `skeleton(src) -> String`: mantém `use`, assinaturas de fn, structs/enums/traits completos (campos), `impl` com só assinaturas; corpos viram `{ … }`; doc comments opcionais.
   - `list_symbols(src)` com caminhos `mod::Tipo::metodo`, ranges de bytes.
   - `read_symbol(src, path)`; `replace_symbol(src, path, new_code)` / `insert_symbol(src, after_path, code)`: troca de range no texto original → reparse → rejeita se `parse.errors()` novos → `rustfmt --edition 2024` via stdin.
   - Testes: snapshots de skeleton, substituição, rejeição de edição inválida, arquivo com erro de sintaxe ainda gera skeleton.
2. **coturbo-providers**
   - Tipos neutros: `Message`, `ContentBlock` (text/tool_use/tool_result), `ToolSpec`, `StreamEvent`, `Usage { input, cache_read, cache_write, output }`.
   - `anthropic`: Messages API streaming; `cache_control: ephemeral` no system, último tool e última mensagem do usuário.
   - `openai_compat`: chat/completions streaming com tool_calls; `base_url` configurável (OpenAI, OpenRouter, Ollama); mapeia `prompt_tokens_details.cached_tokens`.
   - `pricing.rs` lê `pricing.toml`. Backoff limitado em 429/5xx, erro visível.
   - Testes com fixtures SSE gravadas (sem rede).
3. **coturbo-tools**
   - `ToolSet::Naive`: read_file, write_file, grep, bash.
   - `ToolSet::Ast`: skeleton, read_symbol, replace_symbol, insert_symbol, grep, bash, cargo_check (`--message-format=json` → só `arquivo:linha: mensagem` + trecho, máx N erros).
   - Paths confinados à raiz do projeto; erro de ferramenta vira tool_result de erro.
4. **coturbo-agent**
   - Loop: envia → stream → executa tool_uses → repete até sem tools ou `max_turns`.
   - `ContextManager`: prefixo estável (system+tools ordenados deterministicamente); compactação determinística: leituras de símbolo/arquivo editado depois → `[desatualizado: releia]`; outputs antigos de cargo_check substituídos quando há novo.
   - Acumula `Usage` por turno/sessão. Teste e2e com `MockProvider` roteirizado.
5. **coturbo-cli**
   - `coturbo` (REPL) com status: `tokens ↑in ⟳cache ↓out · $custo`.
   - `coturbo run --provider --model --tools naive|ast "tarefa"`.
   - `coturbo bench --tasks bench/tasks --modes naive,ast`: copia fixture para tempdir por execução, roda, executa `verify`, grava `bench/runs/<ts>.json` + tabela Markdown (tokens, custo, turnos, passou).
   - Config `~/.config/coturbo/config.toml` (Windows: `%APPDATA%\coturbo`), chaves via env.
6. **Benchmark fixture + 5–8 tarefas** (ex.: adicionar método, corrigir bug com teste falhando, renomear campo em vários arquivos, adicionar variante de enum + match, refatorar função longa).

## Fora do escopo do MVP
TUI ratatui, outras linguagens, OAuth/assinaturas, licença Pro, roteamento Operário/Arquiteto, servidor LSP, REA no produto.

## Verificação
- `cargo test --workspace` verde (ast, providers com SSE gravado, agent com mock).
- `cargo clippy --workspace -- -D warnings`.
- Manual: `coturbo run --tools ast "adicione um método X"` no fixture com `ANTHROPIC_API_KEY`; conferir edição, cargo test, status de tokens.
- `coturbo bench` com um modelo real: relatório mostra tokens/custo naive vs ast e taxa de sucesso — este é o critério de validação da ideia (A).
