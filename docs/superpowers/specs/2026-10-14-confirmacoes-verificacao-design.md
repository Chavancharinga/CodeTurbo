# Subprojeto 1 — Confirmações, modo plano e verificação real

Data: 2026-10-14. Status: implementado (96 testes, clippy limpo); não commitado.

## Contexto

O CoTurbo (Fases 1–5 da spec do MVP) já tem motor econômico (AST, cache, compactação), provedores por API e um REPL simples, mas executa toda ferramenta sem perguntar e só "verifica" quando o modelo decide chamar `cargo_check`.

Decisões do usuário em 2026-10-14:
- Roteiro: (1) confirmações + modo plano + verificação, (2) TUI ratatui, (3) provedores (API + login por assinatura opcional), (4) benchmark (Fase 6), (5) motor de design embutido (Fase 7 reescrita, 100% embutido, externos opcionais).
- Arquitetura: núcleo por eventos; aprovações e verificação vivem no motor e valem para qualquer interface.
- Confirmações e verificação **exatamente como o oh-my-pi (omp v18.4.12)**, mais o portão de build nosso por cima. Referência lida no código instalado: `pi-coding-agent/src/tools/approval.ts`, `src/plan-mode/`, `src/modes/interactive-mode.ts`, `src/lsp/writethrough.ts`, `src/lsp/workspace-diagnostics.ts`. Copiamos comportamento, não código.
- "Nada de testar entre aspas": a tarefa só termina quando o projeto compila, passa lint e testes de verdade, sem nenhum erro **nem alerta**.

## Objetivo e critérios de sucesso

1. Nenhuma ferramenta acima do nível permitido roda sem decisão explícita (política do usuário ou resposta à pergunta).
2. Modo plano com o mesmo fluxo do omp, sem alterar a lista de ferramentas (prefixo de cache intacto).
3. Erros e alertas aparecem no resultado da própria edição (camada 1), o projeto inteiro pode ser checado (camada 2), e nenhuma tarefa é reportada como concluída sem o portão final passar (camada 3).
4. `cargo test --workspace` e `cargo clippy --workspace --all-targets -- -D warnings` verdes.

## 1. Modelo de permissões

### Nível por ferramenta (fixo no código)

| Nível | Ferramentas |
|---|---|
| `read` | `read_file`, `read_symbol`, `list_files`, `grep`, `workspace_diagnostics` (substitui `cargo_check`) |
| `write` | `write_file`, `edit_file`, `replace_symbol`, `insert_symbol` |
| `exec` | `bash`; qualquer ferramenta futura sem nível declarado (MCP, terceiros) |

`propose_plan` é `read` (só tem efeito em modo plano).

### Modos (iguais ao omp)

| Modo | Aprova sem perguntar até |
|---|---|
| `always-ask` | `read` |
| `write` (**padrão do CoTurbo**; o omp usa `yolo`) | `write` |
| `yolo` | `exec` |

`--auto-approve` força `yolo`.

### Políticas no `config.toml`

```toml
approval_mode = "write"

[approval]
bash = "prompt"                 # allow | deny | prompt
write_file = "allow"
"bash:cargo test*" = "allow"    # padrão por comando (extensão do CoTurbo)
```

- Chave `"bash:<glob>"` casa com o comando inteiro (`*` = qualquer sequência). Entre várias, vence a mais longa; empate → `deny` > `prompt` > `allow`.
- Valor inválido é erro de carregamento do config, não ignorado.

### Ordem de resolução — `resolve(tool, input, config, sessão) -> Allow | Deny(motivo) | Prompt`

1. Modo plano ativo e nível > `read` (exceto escrita no próprio arquivo do plano) → `Deny("modo plano: somente leitura")`.
2. Padrão `bash:<glob>` do usuário, se casar.
3. Política do usuário para o nome da ferramenta.
4. Aprovação de sessão (ver abaixo) → `Allow`.
5. Nível ≤ teto do modo → `Allow`; senão `Prompt`.

`deny` do usuário vale também em `yolo` (como no omp). `allow` do usuário vale também em `always-ask`.

### Pergunta de aprovação

Mostra ferramenta + argumento principal; para ferramentas de escrita, o diff unificado que seria aplicado (via `preview`, simulação sem escrever). Opções:

- **Allow once** — só esta chamada.
- **Always for this session** — grava na sessão: ferramentas de escrita por nome; `bash` pelo comando-base (primeira palavra, ex.: `cargo`). Some ao sair.
- **Deny** — com motivo opcional; volta ao modelo como `tool_result` de erro: `negado pelo usuário: <motivo>`.

As permissões **nunca alteram a lista de ferramentas** enviada ao modelo.

## 2. Verificação real (três camadas)

### Camada 1 — diagnóstico a cada edição (igual a `lsp.diagnosticsOnWrite` / `lsp.formatOnWrite` do omp)

- Após cada ferramenta de escrita bem-sucedida: formata o arquivo e anexa ao resultado os erros e alertas **daquele arquivo**, formato `arquivo:linha:col nível mensagem` + linha de código, no máximo 20.
- Fonte: servidores de linguagem externos via LSP sobre stdio (cliente próprio em Rust). Padrões: `rust-analyzer` (`Cargo.toml`), `typescript-language-server --stdio` (`tsconfig.json`/`package.json`), configuráveis em `[lsp.servers.<nome>] command, args, root_markers, extensions`.
- Temporização como no omp: espera inline curta (padrão 1500 ms) por um `publishDiagnostics` com versão ≥ à da edição. Sem resposta a tempo, a edição retorna sem diagnóstico e o resultado tardio é anexado ao **próximo** `tool_result` do turno seguinte (`[diagnóstico tardio] …`). Nunca bloqueia.
- Config: `diagnostics_on_write = true`, `format_on_write = true`. Para `.rs`, a formatação continua sendo `rustfmt` (já usado pelas ferramentas AST).

### Camada 2 — `workspace_diagnostics` (igual ao workspace diagnostics do omp)

- Detecta **todos** os marcadores na raiz e roda cada verificador: `cargo check --message-format=json` (Rust), `npx tsc --noEmit` (TS), `go build ./...` (Go; `go.work` tem prioridade sobre `go.mod`), `pyright` (Python; `pyproject.toml` tem prioridade sobre `pyrightconfig.json`).
- No máximo 2 verificadores em paralelo; saída em ordem de detecção, com cabeçalho por linguagem quando houver mais de uma.
- Saída vazia com código de saída ≠ 0 é falha do verificador, nunca "limpo".
- Ferramenta de nível `read` exposta ao modelo; substitui `cargo_check` e, como ele, só existe no conjunto `ast`. As camadas 1 e 3 valem nos dois conjuntos (`naive` e `ast`), para o A/B continuar justo.

### Camada 3 — portão final de build (do CoTurbo)

Quando o modelo encerra o turno sem chamar ferramentas **e** houve edição na tarefa, o motor executa, parando na primeira etapa que falhar:

| Detectado | Etapas |
|---|---|
| `Cargo.toml` | `cargo build --all-targets --message-format=json` → `cargo clippy --all-targets --message-format=json -- -D warnings` → `cargo test` |
| `package.json` | scripts existentes, nesta ordem: `typecheck` (ou `npx tsc --noEmit` se houver `tsconfig.json`) → `lint` → `build` → `test` |
| `go.mod` | `go build ./...` → `go vet ./...` → `go test ./...` |
| `pyproject.toml` | `pyright` → `pytest` (se instalado) |
| override | `verify = ["cmd1", "cmd2"]` em `config.toml` ou `.coturbo.toml` do projeto substitui a detecção |

Etapa opcional `run = "cargo run -- --help"` (timeout 60 s): falha com exit ≠ 0, `panicked at`, ou stack trace.

- **Critério: zero erros e zero alertas.** Alertas do compilador/clippy/tsc contam como falha.
- Sucesso → o modelo recebe uma linha: `✓ build, clippy, test: 0 erros, 0 alertas`, e a tarefa termina `verified = true`.
- Falha → o modelo recebe só diagnósticos compactos (deduplicados, máx. 20; testes: nome do teste + mensagem de falha + localização) e o loop continua. Até `max_fix_rounds = 3` rodadas; esgotadas, a tarefa termina `verified = false`, exibido como **"não verificado"**.
- Comandos do portão vêm do config/detecção, não do modelo: rodam sem pergunta. Exceção: `verify`/`run` vindos de `.coturbo.toml` do projeto pedem confirmação uma vez por sessão (repositório pode não ser confiável).
- `final_gate = false` desliga a camada (comportamento omp puro). Timeout por etapa: 600 s.

### Anti-trapaça

Snapshot dos arquivos editados antes da primeira edição da tarefa vs. estado atual, avaliado antes do portão. Detecta:

- queda no número de testes (Rust: fns com `#[test]`/`#[tokio::test]` via `ra_ap_syntax`; JS/TS: chamadas `it(`/`test(`; Python: `def test_`);
- `#[ignore]`, `.skip(`, `xit(`, `@pytest.mark.skip` novos;
- `#[allow(`, `#![allow(`, `eslint-disable`, `@ts-ignore`, `@ts-expect-error`, `# type: ignore` novos;
- `todo!()`, `unimplemented!()` novos;
- teste existente cujo corpo ficou vazio ou só com `assert!(true)` / `expect(true)`.

Cada achado vira uma pergunta ao usuário (aceitar / rejeitar com motivo). Rejeitado → volta ao modelo como falha do portão. Em `yolo`/bench, achados reprovam direto.

## 3. Modo plano (igual ao omp)

- Entrada: `/plan` no REPL/TUI ou `--plan` no início. Modelo de planejamento opcional: `--plan-model <id>` (como `--plan` do omp); a execução usa `--model`.
- Durante: tudo acima de `read` é negado pelo motor, exceto escrita em `.coturbo/plans/<slug>-plan.md`. A regra entra como aviso curto anexado à mensagem do usuário; lista de ferramentas inalterada.
- Instruções ao modelo (resumo do prompt do omp): plano é spec executável, sem decisões abertas; toda afirmação verificada nesta sessão ou marcada `não verificado`; escrever o arquivo incrementalmente; pedir aprovação **só** chamando `propose_plan(slug)`.
- `propose_plan` mostra o plano no `Host::review_plan` com as 5 opções:
  1. **Aprovar e executar** — histórico zerado; nova tarefa só com o conteúdo do plano.
  2. **Aprovar e compactar** — `compact()` e executa com o plano.
  3. **Aprovar mantendo o contexto** — desabilitada quando o contexto estimado passa de 95% da janela do modelo.
  4. **Refinar plano** — comentário do usuário volta como `tool_result`; continua em modo plano.
  5. **Salvar e sair** — mantém o arquivo, sai do modo plano sem executar.
- Na revisão (TUI, subprojeto 2): apagar/desfazer seções e abrir em `$EDITOR`. No REPL: mostra o markdown e um menu numerado; `e` abre `$EDITOR`.
- Após aprovar, o modo de aprovação normal volta a valer; as três camadas de verificação continuam ativas.
- Fora deste subprojeto: `--plan-yolo`, `prewalk`.

## 4. Arquitetura

| Unidade | Local | Responsabilidade |
|---|---|---|
| `approval` | `coturbo-agent/src/approval.rs` | `Tier`, `ApprovalMode`, `Policy`, padrões `bash:`, `SessionGrants`, `resolve()` pura |
| `Host` | `coturbo-agent/src/lib.rs` | Substitui `Observer`: `on_event(&Event)`, `approve(&ApprovalRequest) -> BoxFuture<Decision>`, `review_plan(&Plan) -> BoxFuture<PlanChoice>`, `confirm_cheat(&Finding) -> BoxFuture<bool>` |
| `plan` | `coturbo-agent/src/plan.rs` | estado do modo plano, ferramenta `propose_plan`, aplicação das 5 escolhas |
| `tier()`, `preview()` | `coturbo-tools` | nível por nome; diff da edição sem gravar |
| `coturbo-verify` (novo crate) | `lsp/` cliente JSON-RPC stdio + gerenciador de servidores · `workspace.rs` camada 2 · `gate.rs` camada 3 · `cheat.rs` anti-trapaça · `diag.rs` formato compacto comum |

`Event`: `Text(delta)`, `ToolStart{name,input}`, `ToolEnd{name,output}`, `Turn{turn_usage,task_usage}`, `Gate{stage,ok,summary}`, `Notice(String)`.

`TaskReport` ganha `verified: Option<bool>` (`None` = nenhuma edição, portão não rodou).

### Hosts

- **REPL** (até a TUI): pergunta no terminal com `[o]nce / [s]ession / [d]eny`; motivo opcional após `d`.
- **`coturbo run`**: com TTY, igual ao REPL; sem TTY, `Prompt` vira `Deny("sem terminal; use --auto-approve")`, plano é rejeitado, anti-trapaça reprova.
- **bench**: `yolo`, portão ligado, `verified` no JSON e na tabela.

### Erros

- Servidor LSP ausente → um `Notice` por sessão; camadas 2 e 3 seguem.
- LSP travado → timeout (10 s por pedido), reinício na próxima edição; diagnóstico vira tardio.
- Comando do portão inexistente → `verified = false` com o motivo.
- Ctrl+C durante pergunta → `Deny("cancelado")`.
- Erro de config (política inválida, glob vazio) → falha ao iniciar com mensagem clara.

## 5. Testes (TDD)

- `resolve`: tabela modo × nível × política; `deny` vencendo em `yolo`; `allow` em `always-ask`; padrões `bash:` (mais longo vence, empate pelo mais restritivo); sessão; modo plano.
- Agente com `Mock` + `Host` roteirizado: negação com motivo chega como `tool_result` de erro; "sessão" evita segunda pergunta; modo plano bloqueia escrita e permite o arquivo do plano; as 5 escolhas; portão falha → modelo corrige → `verified = true`; rodadas esgotadas → `verified = false`; lista de ferramentas idêntica em todos os requests.
- LSP: binário de teste `fake-lsp` (Rust) com modos rápido, lento (tardio) e crash.
- Portão: crate real em tempdir com alerta proposital (reprova), corrigida (passa); teste que falha (reprova com nome do teste).
- Workspace: raiz com `Cargo.toml` + `tsconfig.json` detecta ambos; saída vazia com exit ≠ 0 é falha.
- Anti-trapaça: um caso por padrão listado.

## 6. Ordem de implementação

1. `approval` + `Host` + REPL/`run` com perguntas.
2. Modo plano.
3. Portão final + anti-trapaça.
4. `workspace_diagnostics` (substitui `cargo_check`).
5. LSP na edição.

## Fora do escopo

TUI ratatui (subprojeto 2), novos provedores/login (3), `--plan-yolo`, `prewalk`, aprovação persistente entre sessões, sandbox de sistema para `bash`.

## Notas de implementação (2026-10-14)

- `bench` roda sem LSP (resultado determinístico e sem custo de indexação por cópia da fixture); portão final ligado em ambos os modos.
- Etapas de shell do portão (JS/Go/Python/`verify`) detectam alertas por heurística de linha (`warning`), ignorando `npm warn` e resumos "0 warnings".
- Anti-trapaça compara só arquivos tocados pelas ferramentas de edição; mudanças feitas via `bash` não entram no snapshot.
- `format_on_write` em `.rs` (`write_file`/`edit_file`) só aplica `rustfmt` se o arquivo já estava formatado antes — nunca reformata um arquivo que não seguia o rustfmt.
- Diagnóstico que não chega na janela inline responde "pending" e o resultado tardio vem como `[late] …` no próximo `tool_result`; servidor ausente é avisado uma vez (`[lsp] … unavailable`).
- Testes de LSP usam o binário `coturbo-fake-lsp` (crate `coturbo-verify`, modos `fast|slow|unversioned|crash`).
- Abertura animada "CodeTurbo" (`coturbo splash`, e no início do REPL em terminal).
